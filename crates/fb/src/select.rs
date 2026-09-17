//! `fb select` — choose which arms attempt a task, by Thompson sampling.
//!
//! Gives `farmerbob_core::router` its first caller. The router has existed, tested and
//! unreachable, for the whole project; meanwhile arms were hand-picked, and the same four
//! rotated through seven consecutive waves while three free routes went untried — one of them
//! 100% complete over five runs and sitting on the Pareto frontier. On a project whose entire
//! output is a comparison across arms, hand-picking quietly narrows the comparison to whoever
//! won first, which is a measurement that cannot contradict itself.
//!
//! It also gives `router::eligibility` its first caller, and with it the paid-duplicate guard
//! that farmerbob-vgn recorded as "wired to nothing": a paid route to a model already
//! reachable free is excluded, using `pricing::canonical_model` to decide when two route ids
//! reach the same weights.
//!
//! Eligibility is a FILTER and never evidence. An arm parked by a rate limit, or lacking a
//! capability, or redundant with a free twin, is skipped without its posterior being touched —
//! a transient outage must not permanently demote a good arm.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use farmerbob_core::pricing::canonical_model;
use farmerbob_core::prior::{update, Posterior};
use farmerbob_core::router::{eligibility, select, ArmInfo, ArmName, Weights};

/// Deterministic PRNG. The draws that pick a wave's field are reproducible from its seed, so
/// a selection can be replayed and argued with, like every other measurement here.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).max(1))
    }
    /// xorshift64*, uniform in (0, 1).
    fn uniform(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // Keep it strictly inside (0,1): ln(0) and 1.0 both break the samplers below.
        ((v >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
    /// Marsaglia-Tsang gamma. Shape below 1 is boosted and corrected.
    fn gamma(&mut self, shape: f64) -> f64 {
        if shape < 1.0 {
            let g = self.gamma(shape + 1.0);
            return g * self.uniform().powf(1.0 / shape);
        }
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            let x = self.normal();
            let v = 1.0 + c * x;
            if v <= 0.0 {
                continue;
            }
            let v = v * v * v;
            let u = self.uniform();
            if u < 1.0 - 0.0331 * x.powi(4) || u.ln() < 0.5 * x * x + d * (1.0 - v + v.ln()) {
                return d * v;
            }
        }
    }
    /// Beta(a, b) as the ratio of two gammas. Always lands in [0, 1].
    fn beta(&mut self, a: f64, b: f64) -> f64 {
        let (x, y) = (self.gamma(a.max(1e-9)), self.gamma(b.max(1e-9)));
        let s = x + y;
        if s.is_finite() && s > 0.0 { (x / s).clamp(0.0, 1.0) } else { 0.5 }
    }
}

/// One arm as the registry describes it, before any history is folded in.
struct Registered {
    info: ArmInfo,
    model: String,
    free: bool,
    /// The registry records nothing about this arm's capabilities.
    capability_unknown: bool,
}

fn registry(repo: &Path, needed_all: &[String]) -> Result<Vec<Registered>, String> {
    let body = fs::read_to_string(repo.join("sources.toml"))
        .map_err(|e| format!("cannot read sources.toml: {e}"))?;
    let doc: toml::Value =
        toml::from_str(&body).map_err(|e| format!("sources.toml is not valid TOML: {e}"))?;
    let table = doc
        .get("source")
        .and_then(|s| s.as_table())
        .ok_or_else(|| "sources.toml has no [source] table".to_string())?;

    let mut out = Vec::new();
    for (name, v) in table {
        let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
        let disabled_reason = match v.get("status").and_then(|s| s.as_str()) {
            Some("disabled") => Some(
                v.get("disabled_reason")
                    .and_then(|r| r.as_str())
                    .unwrap_or("disabled in the registry")
                    .to_string(),
            ),
            _ => None,
        };
        // A capability the registry does not RECORD is unknown, not absent. Only 2 of 24
        // arms carry a `multiturn` key; the first draft of this function turned the other 22
        // into MissingCapability and benched them, which is this project's own recurring bug
        // committed inside the fix for a different instance of it.
        //
        // So: an arm with NO recorded capability data is not excludable on capability
        // grounds. It is reported as unknown instead, which is a gap in the registry rather
        // than a fact about the arm.
        let recorded = v.get("multiturn").and_then(toml::Value::as_bool);
        let capabilities: Vec<String> = match recorded {
            Some(true) => vec!["multiturn".to_string()],
            Some(false) => Vec::new(),
            // Unknown: satisfy any requirement, and say so in the report.
            None => needed_all.to_vec(),
        };
        let capability_unknown = recorded.is_none();
        out.push(Registered {
            capability_unknown,
            free: model.ends_with(":free")
                || v.get("quota").and_then(|q| q.as_str()) == Some("plan"),
            model,
            info: ArmInfo {
                name: ArmName(name.clone()),
                // Replaced below from measured history; uniform until then, which is what
                // makes an untried arm win sometimes.
                posterior: Posterior { alpha: 1.0, beta: 1.0 },
                price_in: v.get("price_in").and_then(toml::Value::as_float).unwrap_or(0.0),
                mean_latency_s: None,
                capabilities,
                parked_until: v.get("parked_until").and_then(toml::Value::as_integer).map(|i| i as u64),
                redundant_with: v.get("redundant_with").and_then(|r| r.as_str()).map(str::to_string),
                disabled_reason,
            },
        })
    }
    out.sort_by(|a, b| a.info.name.as_str().cmp(b.info.name.as_str()));
    Ok(out)
}

/// Successes, failures and mean latency per arm, from runs that say something about the arm.
///
/// Only `outcome == "arm_result"` counts. An infrastructure fault, a quota refusal or an
/// orchestrator kill is not evidence, and folding it in would let an outage demote a good arm.
fn history(base: &Path) -> BTreeMap<String, (u32, u32, Option<f64>)> {
    let mut acc: BTreeMap<String, (u32, u32, f64, u32)> = BTreeMap::new();
    let Ok(body) = fs::read_to_string(base.join("logs/objective.json")) else {
        return BTreeMap::new();
    };
    let rows: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap_or_default();
    for r in &rows {
        if r.get("outcome").and_then(|o| o.as_str()) != Some("arm_result") {
            continue;
        }
        let Some(arm) = r.get("arm").and_then(|a| a.as_str()) else { continue };
        let e = acc.entry(arm.to_string()).or_insert((0, 0, 0.0, 0));
        if r.get("verdict").and_then(|v| v.as_str()) == Some("PASS") {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
        if let Some(s) = r.get("secs").and_then(serde_json::Value::as_f64) {
            e.2 += s;
            e.3 += 1;
        }
    }
    acc.into_iter()
        .map(|(k, (s, f, secs, n))| {
            (k, (s, f, (n > 0).then(|| secs / f64::from(n))))
        })
        .collect()
}

pub fn run_cmd(n: usize, seed: Option<u64>, needed: &[String], json_only: bool) -> i32 {
    let repo = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let base = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h).join(".local/share/farmerbob"),
        Err(_) => return 2,
    };
    let mut reg = match registry(&repo, needed) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let hist = history(&base);
    for r in &mut reg {
        if let Some((s, f, lat)) = hist.get(r.info.name.as_str()) {
            r.info.posterior = update(r.info.posterior, *s, *f);
            r.info.mean_latency_s = *lat;
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // A paid route is redundant only when a FREE route to the same weights is itself healthy.
    // canonical_model is what decides "same weights": it strips the :free suffix and the
    // provider prefix, so openrouter/x/y:free and openrouter/x/y collapse together.
    let healthy_free: Vec<String> = reg
        .iter()
        .filter(|r| r.free && r.info.disabled_reason.is_none())
        .filter(|r| r.info.parked_until.is_none_or(|u| now >= u))
        .map(|r| r.info.name.as_str().to_string())
        .collect();
    let free_models: Vec<String> = reg
        .iter()
        .filter(|r| r.free && r.info.disabled_reason.is_none())
        .map(|r| canonical_model(&r.model))
        .collect();
    for r in &mut reg {
        if !r.free && r.info.redundant_with.is_none() {
            let canon = canonical_model(&r.model);
            if let Some(pos) = free_models.iter().position(|m| *m == canon) {
                // Name the free arm that covers it, which is what Ineligible reports.
                r.info.redundant_with = reg_name_at(&healthy_free, pos);
            }
        }
    }

    let seed = seed.unwrap_or(now);
    let mut rng = Rng::new(seed);
    let weights = Weights { task_value: 1.0, dollar_weight: 0.02, latency_weight: 0.00005 };

    let mut pool: Vec<ArmInfo> = reg.iter().map(|r| r.info.clone()).collect();
    let mut chosen: Vec<(String, f64, f64)> = Vec::new();
    while chosen.len() < n {
        let draws: Vec<f64> = pool
            .iter()
            .filter(|a| eligibility(a, needed, now, &healthy_free).is_none())
            .map(|a| rng.beta(a.posterior.alpha, a.posterior.beta))
            .collect();
        let Some(c) = select(&pool, needed, now, &healthy_free, &weights, &draws) else {
            break;
        };
        chosen.push((c.arm.as_str().to_string(), c.sampled_p, c.score));
        pool.retain(|a| a.name.as_str() != c.arm.as_str());
    }

    if json_only {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "seed": seed,
                "chosen": chosen.iter().map(|(a, p, s)| serde_json::json!({
                    "arm": a, "sampled_p": p, "score": s
                })).collect::<Vec<_>>(),
            }))
            .unwrap_or_else(|_| "{}".into())
        );
        return 0;
    }

    println!("seed {seed}  (selection is reproducible from it)\n");
    println!("{:<22}{:>9}{:>8}{:>9}{:>8}  {}", "CHOSEN", "DRAWN p", "SCORE", "MEAN", "RUNS", "");
    for (arm, p, score) in &chosen {
        let post = reg
            .iter()
            .find(|r| r.info.name.as_str() == arm)
            .map(|r| r.info.posterior)
            .unwrap_or(Posterior { alpha: 1.0, beta: 1.0 });
        println!(
            "{arm:<22}{p:>9.3}{score:>8.3}{:>9.2}{:>8.0}",
            post.mean(),
            post.observations()
        );
    }
    let unknown: Vec<&str> = reg.iter().filter(|r| r.capability_unknown)
        .map(|r| r.info.name.as_str()).collect();
    if !unknown.is_empty() && !needed.is_empty() {
        println!(
            "\n{} of {} arms have no recorded capabilities, so the {:?} requirement could not",
            unknown.len(), reg.len(), needed
        );
        println!("  be applied to them. That is a gap in sources.toml, not a fact about them.");
    }
    println!("\n{:<22}{}", "EXCLUDED", "why  (a filter, never evidence -- no posterior moved)");
    for r in &reg {
        if let Some(reason) = eligibility(&r.info, needed, now, &healthy_free) {
            println!("{:<22}{reason:?}", r.info.name.as_str());
        }
    }
    if chosen.is_empty() {
        eprintln!("\nno eligible arm");
        return 3;
    }
    println!("\ntsv: {}", chosen.iter().map(|(a, _, _)| a.as_str()).collect::<Vec<_>>().join(","));
    0
}

/// The free arm at `pos` in the healthy list, if the lists still line up.
fn reg_name_at(healthy: &[String], pos: usize) -> Option<String> {
    healthy.get(pos).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_are_reproducible_from_the_seed() {
        let a: Vec<f64> = { let mut r = Rng::new(42); (0..8).map(|_| r.uniform()).collect() };
        let b: Vec<f64> = { let mut r = Rng::new(42); (0..8).map(|_| r.uniform()).collect() };
        assert_eq!(a, b, "a selection must be replayable from its seed");
        let c: Vec<f64> = { let mut r = Rng::new(43); (0..8).map(|_| r.uniform()).collect() };
        assert_ne!(a, c, "different seeds must explore differently");
    }

    #[test]
    fn uniforms_stay_strictly_inside_the_open_interval() {
        // ln(0) is -inf and 1.0 breaks the gamma rejection loop; neither may ever appear.
        let mut r = Rng::new(7);
        for _ in 0..20_000 {
            let u = r.uniform();
            assert!(u > 0.0 && u < 1.0, "uniform escaped (0,1): {u}");
        }
    }

    #[test]
    fn beta_draws_are_finite_and_in_range_including_degenerate_shapes() {
        let mut r = Rng::new(11);
        for (a, b) in [(1.0, 1.0), (0.001, 0.001), (500.0, 1.0), (1.0, 500.0), (0.0, 0.0)] {
            for _ in 0..500 {
                let p = r.beta(a, b);
                assert!(p.is_finite(), "Beta({a},{b}) produced a non-finite draw");
                assert!((0.0..=1.0).contains(&p), "Beta({a},{b}) produced {p}");
            }
        }
    }

    /// A strongly-evidenced arm should usually out-draw a weakly-evidenced one, but not
    /// always: that "not always" is the exploration the whole mechanism exists for.
    #[test]
    fn a_proven_arm_usually_but_not_always_beats_an_unknown_one() {
        let mut r = Rng::new(3);
        let (mut proven_wins, n) = (0, 2_000);
        for _ in 0..n {
            let proven = r.beta(19.0, 3.0); // 18 successes, 2 failures
            let unknown = r.beta(1.0, 1.0); // never run
            if proven > unknown {
                proven_wins += 1;
            }
        }
        assert!(proven_wins > n * 6 / 10, "the proven arm should usually win: {proven_wins}/{n}");
        assert!(proven_wins < n, "an unknown arm must sometimes win, or nothing is explored");
    }

    /// An unrecorded capability is UNKNOWN, and must not read as absent. Getting this
    /// backwards benched 22 of 24 arms on missing registry metadata.
    #[test]
    fn an_unrecorded_capability_does_not_exclude_an_arm() {
        let needed = vec!["multiturn".to_string()];
        let unknown_caps = needed.clone(); // what registry() gives an arm with no record
        let info = ArmInfo {
            name: ArmName("unrecorded".into()),
            posterior: Posterior { alpha: 1.0, beta: 1.0 },
            price_in: 0.0,
            mean_latency_s: None,
            capabilities: unknown_caps,
            parked_until: None,
            redundant_with: None,
            disabled_reason: None,
        };
        assert!(
            eligibility(&info, &needed, 0, &[]).is_none(),
            "an arm the registry says nothing about must not be excluded for lacking it"
        );
    }
}
