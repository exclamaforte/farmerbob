//! Compare registered arm prices against a model catalogue.
//!
//! OpenRouter and other model providers periodically update token prices.
//! This module compares the prices currently recorded in the registry
//! against a fetched catalogue to decide which arms require edits, what the
//! new values are, and whether an arm's model is unquoted.
//!
//! This decision is pure: no network, no I/O, no TOML parsing or regexes.
//! A model absent from the catalogue is reported as [`Change::Unquoted`]
//! and is never treated as free, ensuring unquoted models cannot fabricate
//! a zero cost on the Pareto frontier.

use crate::measurement::Measurement;

/// What the registry holds for one arm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    /// The arm's registry name.
    pub arm: String,
    /// The model id, e.g. `openrouter/inclusionai/ling-3.0-flash-vl:free`.
    pub model: String,
    /// Price per million input tokens, in millionths of a unit so this type
    /// stays `Eq`. `Missing` when the registry states none.
    pub price_in: Measurement<u64>,
    /// Price per million output tokens, same units.
    pub price_out: Measurement<u64>,
}

/// What the catalogue says, keyed the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quoted {
    /// The model id as the catalogue spells it.
    pub model: String,
    /// Catalogue price per million input tokens.
    pub price_in: u64,
    /// Catalogue price per million output tokens.
    pub price_out: u64,
}

/// One arm's price compared against the catalogue. Exactly these and no others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The registry and the catalogue agree.
    Agrees,
    /// They differ, and this is the new pair.
    Differs {
        /// New input price.
        price_in: u64,
        /// New output price.
        price_out: u64,
    },
    /// The registry states no price and the catalogue does. Filling a blank
    /// is not the same as correcting a figure.
    Fills {
        /// The input price to fill in.
        price_in: u64,
        /// The output price to fill in.
        price_out: u64,
    },
    /// The catalogue does not list this model. NEVER treated as free.
    Unquoted,
}

/// Compare one arm against the catalogue.
///
/// Looks up the arm's model in `catalogue` by exact string match. If the model
/// is absent from the catalogue, returns [`Change::Unquoted`].
///
/// If duplicate entries for the same model exist in `catalogue`, the first
/// entry wins and later duplicates are ignored.
///
/// When the model is found:
/// - If both registry prices are observed and equal the catalogue prices,
///   returns [`Change::Agrees`].
/// - If both registry prices are missing, returns [`Change::Fills`] carrying
///   the catalogue's price pair.
/// - If one registry price is missing and the other is observed, returns
///   [`Change::Differs`] carrying the catalogue's price pair.
/// - Otherwise (both observed but at least one differs), returns
///   [`Change::Differs`] carrying the catalogue's price pair.
pub fn compare(reg: &Registered, catalogue: &[Quoted]) -> Change {
    let Some(quoted) = catalogue.iter().find(|q| q.model == reg.model) else {
        return Change::Unquoted;
    };

    match (&reg.price_in, &reg.price_out) {
        (Measurement::Observed(in_p), Measurement::Observed(out_p)) => {
            if *in_p == quoted.price_in && *out_p == quoted.price_out {
                Change::Agrees
            } else {
                Change::Differs {
                    price_in: quoted.price_in,
                    price_out: quoted.price_out,
                }
            }
        }
        (Measurement::Missing(_), Measurement::Missing(_)) => Change::Fills {
            price_in: quoted.price_in,
            price_out: quoted.price_out,
        },
        (Measurement::Observed(_), Measurement::Missing(_))
        | (Measurement::Missing(_), Measurement::Observed(_)) => Change::Differs {
            price_in: quoted.price_in,
            price_out: quoted.price_out,
        },
    }
}

/// Every arm that needs an edit, in the order given. `Agrees` and
/// `Unquoted` are not edits and do not appear.
///
/// Returns one entry per arm needing an edit in the order `regs` was given,
/// with no deduplication and no sorting.
pub fn pending(regs: &[Registered], catalogue: &[Quoted]) -> Vec<(String, Change)> {
    let mut edits = Vec::new();
    for reg in regs {
        let change = compare(reg, catalogue);
        match change {
            Change::Agrees | Change::Unquoted => {}
            Change::Differs { .. } | Change::Fills { .. } => {
                edits.push((reg.arm.clone(), change));
            }
        }
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Absent;

    #[test]
    fn clause_1_equal_prices_agree() {
        let reg = Registered {
            arm: "claude-haiku".to_string(),
            model: "anthropic/claude-3-haiku".to_string(),
            price_in: Measurement::Observed(250_000),
            price_out: Measurement::Observed(1_250_000),
        };
        let cat = [Quoted {
            model: "anthropic/claude-3-haiku".to_string(),
            price_in: 250_000,
            price_out: 1_250_000,
        }];
        assert_eq!(compare(&reg, &cat), Change::Agrees);
    }

    #[test]
    fn clause_1_and_boundary_zero_price_agree() {
        let reg = Registered {
            arm: "free-arm".to_string(),
            model: "openrouter/free-model:free".to_string(),
            price_in: Measurement::Observed(0),
            price_out: Measurement::Observed(0),
        };
        let cat = [Quoted {
            model: "openrouter/free-model:free".to_string(),
            price_in: 0,
            price_out: 0,
        }];
        assert_eq!(compare(&reg, &cat), Change::Agrees);
    }

    #[test]
    fn clause_2_different_prices_differ_carrying_catalogue_pair() {
        let reg = Registered {
            arm: "llama".to_string(),
            model: "meta/llama-3".to_string(),
            price_in: Measurement::Observed(100),
            price_out: Measurement::Observed(200),
        };
        let cat = [Quoted {
            model: "meta/llama-3".to_string(),
            price_in: 150,
            price_out: 250,
        }];
        assert_eq!(
            compare(&reg, &cat),
            Change::Differs {
                price_in: 150,
                price_out: 250,
            }
        );
    }

    #[test]
    fn clause_2_single_price_dimension_differs() {
        let reg = Registered {
            arm: "arm-a".to_string(),
            model: "model-a".to_string(),
            price_in: Measurement::Observed(100),
            price_out: Measurement::Observed(200),
        };
        let cat_in_diff = [Quoted {
            model: "model-a".to_string(),
            price_in: 999,
            price_out: 200,
        }];
        assert_eq!(
            compare(&reg, &cat_in_diff),
            Change::Differs {
                price_in: 999,
                price_out: 200,
            }
        );

        let cat_out_diff = [Quoted {
            model: "model-a".to_string(),
            price_in: 100,
            price_out: 999,
        }];
        assert_eq!(
            compare(&reg, &cat_out_diff),
            Change::Differs {
                price_in: 100,
                price_out: 999,
            }
        );
    }

    #[test]
    fn clause_3_missing_both_prices_fills() {
        let reg = Registered {
            arm: "new-arm".to_string(),
            model: "vendor/new-model".to_string(),
            price_in: Measurement::Missing(Absent::NotAttempted),
            price_out: Measurement::Missing(Absent::NotAttempted),
        };
        let cat = [Quoted {
            model: "vendor/new-model".to_string(),
            price_in: 500,
            price_out: 1500,
        }];
        assert_eq!(
            compare(&reg, &cat),
            Change::Fills {
                price_in: 500,
                price_out: 1500,
            }
        );
    }

    #[test]
    fn clause_3_and_boundary_zero_catalogue_price_with_missing_registry_fills_zero() {
        let reg = Registered {
            arm: "unrecorded-free".to_string(),
            model: "vendor/free-route".to_string(),
            price_in: Measurement::Missing(Absent::NothingToMeasure {
                reason: "no previous price".to_string(),
            }),
            price_out: Measurement::Missing(Absent::NothingToMeasure {
                reason: "no previous price".to_string(),
            }),
        };
        let cat = [Quoted {
            model: "vendor/free-route".to_string(),
            price_in: 0,
            price_out: 0,
        }];
        assert_eq!(
            compare(&reg, &cat),
            Change::Fills {
                price_in: 0,
                price_out: 0,
            }
        );
    }

    #[test]
    fn clause_4_unquoted_model_whatever_registry_holds() {
        let cat = [Quoted {
            model: "other/model".to_string(),
            price_in: 100,
            price_out: 200,
        }];

        let reg_with_zeros = Registered {
            arm: "arm-zero".to_string(),
            model: "missing/model".to_string(),
            price_in: Measurement::Observed(0),
            price_out: Measurement::Observed(0),
        };
        assert_eq!(compare(&reg_with_zeros, &cat), Change::Unquoted);

        let reg_with_prices = Registered {
            arm: "arm-priced".to_string(),
            model: "missing/model".to_string(),
            price_in: Measurement::Observed(500),
            price_out: Measurement::Observed(1000),
        };
        assert_eq!(compare(&reg_with_prices, &cat), Change::Unquoted);

        let reg_with_missing = Registered {
            arm: "arm-missing".to_string(),
            model: "missing/model".to_string(),
            price_in: Measurement::Missing(Absent::NotAttempted),
            price_out: Measurement::Missing(Absent::NotAttempted),
        };
        assert_eq!(compare(&reg_with_missing, &cat), Change::Unquoted);
    }

    #[test]
    fn clauses_3_and_4_tested_together() {
        // Both arms hold no usable price in the registry (Missing).
        // Quoted arm must result in Fills; absent arm must result in Unquoted.
        // Treating unquoted as a blank to fill would fabricate an unstated price.
        let quoted_reg = Registered {
            arm: "arm-quoted".to_string(),
            model: "vendor/quoted".to_string(),
            price_in: Measurement::Missing(Absent::NotAttempted),
            price_out: Measurement::Missing(Absent::NotAttempted),
        };
        let unquoted_reg = Registered {
            arm: "arm-unquoted".to_string(),
            model: "vendor/unquoted".to_string(),
            price_in: Measurement::Missing(Absent::NotAttempted),
            price_out: Measurement::Missing(Absent::NotAttempted),
        };
        let cat = [Quoted {
            model: "vendor/quoted".to_string(),
            price_in: 300,
            price_out: 600,
        }];

        assert_eq!(
            compare(&quoted_reg, &cat),
            Change::Fills {
                price_in: 300,
                price_out: 600,
            }
        );
        assert_eq!(compare(&unquoted_reg, &cat), Change::Unquoted);
    }

    #[test]
    fn clause_5_one_missing_price_is_differs_or_fills_not_agrees() {
        let cat = [Quoted {
            model: "vendor/model".to_string(),
            price_in: 100,
            price_out: 200,
        }];

        // Ordering 1: price_in is Missing, price_out is Observed
        let reg_in_missing = Registered {
            arm: "arm-in-missing".to_string(),
            model: "vendor/model".to_string(),
            price_in: Measurement::Missing(Absent::NotAttempted),
            price_out: Measurement::Observed(200),
        };
        let res_in_missing = compare(&reg_in_missing, &cat);
        assert_ne!(res_in_missing, Change::Agrees);
        assert_ne!(res_in_missing, Change::Unquoted);
        assert!(
            matches!(
                res_in_missing,
                Change::Differs {
                    price_in: 100,
                    price_out: 200
                } | Change::Fills {
                    price_in: 100,
                    price_out: 200
                }
            ),
            "expected Differs or Fills carrying catalogue pair, got {:?}",
            res_in_missing
        );

        // Ordering 2: price_in is Observed, price_out is Missing
        let reg_out_missing = Registered {
            arm: "arm-out-missing".to_string(),
            model: "vendor/model".to_string(),
            price_in: Measurement::Observed(100),
            price_out: Measurement::Missing(Absent::NotAttempted),
        };
        let res_out_missing = compare(&reg_out_missing, &cat);
        assert_ne!(res_out_missing, Change::Agrees);
        assert_ne!(res_out_missing, Change::Unquoted);
        assert!(
            matches!(
                res_out_missing,
                Change::Differs {
                    price_in: 100,
                    price_out: 200
                } | Change::Fills {
                    price_in: 100,
                    price_out: 200
                }
            ),
            "expected Differs or Fills carrying catalogue pair, got {:?}",
            res_out_missing
        );
    }

    #[test]
    fn clause_7_exact_model_id_match_free_suffix() {
        let cat = [Quoted {
            model: "provider/model".to_string(),
            price_in: 10,
            price_out: 20,
        }];

        let reg_with_suffix = Registered {
            arm: "arm-free".to_string(),
            model: "provider/model:free".to_string(),
            price_in: Measurement::Observed(0),
            price_out: Measurement::Observed(0),
        };
        assert_eq!(compare(&reg_with_suffix, &cat), Change::Unquoted);

        let cat_with_suffix = [Quoted {
            model: "provider/model:free".to_string(),
            price_in: 0,
            price_out: 0,
        }];
        let reg_without_suffix = Registered {
            arm: "arm-paid".to_string(),
            model: "provider/model".to_string(),
            price_in: Measurement::Observed(10),
            price_out: Measurement::Observed(20),
        };
        assert_eq!(
            compare(&reg_without_suffix, &cat_with_suffix),
            Change::Unquoted
        );
    }

    #[test]
    fn boundary_duplicate_model_in_catalogue_first_entry_wins() {
        let cat = [
            Quoted {
                model: "dup/model".to_string(),
                price_in: 10,
                price_out: 20,
            },
            Quoted {
                model: "dup/model".to_string(),
                price_in: 99,
                price_out: 99,
            },
        ];
        let reg = Registered {
            arm: "dup-arm".to_string(),
            model: "dup/model".to_string(),
            price_in: Measurement::Missing(Absent::NotAttempted),
            price_out: Measurement::Missing(Absent::NotAttempted),
        };
        assert_eq!(
            compare(&reg, &cat),
            Change::Fills {
                price_in: 10,
                price_out: 20,
            }
        );
    }

    #[test]
    fn boundary_empty_catalogue() {
        let cat: [Quoted; 0] = [];
        let reg = Registered {
            arm: "any-arm".to_string(),
            model: "any/model".to_string(),
            price_in: Measurement::Observed(100),
            price_out: Measurement::Observed(200),
        };
        assert_eq!(compare(&reg, &cat), Change::Unquoted);

        let regs = [reg];
        assert_eq!(pending(&regs, &cat), Vec::new());
    }

    #[test]
    fn boundary_empty_regs() {
        let cat = [Quoted {
            model: "any/model".to_string(),
            price_in: 100,
            price_out: 200,
        }];
        let regs: [Registered; 0] = [];
        assert_eq!(pending(&regs, &cat), Vec::new());
    }

    #[test]
    fn clause_6_pending_filtering_and_order_preservation() {
        let cat = [
            Quoted {
                model: "model-a".to_string(),
                price_in: 10,
                price_out: 20,
            },
            Quoted {
                model: "model-b".to_string(),
                price_in: 30,
                price_out: 40,
            },
            Quoted {
                model: "model-c".to_string(),
                price_in: 50,
                price_out: 60,
            },
            Quoted {
                model: "model-d".to_string(),
                price_in: 70,
                price_out: 80,
            },
        ];

        let regs = [
            Registered {
                arm: "arm-1".to_string(),
                model: "model-a".to_string(),
                price_in: Measurement::Observed(10),
                price_out: Measurement::Observed(20),
            }, // Agrees -> omitted
            Registered {
                arm: "arm-2".to_string(),
                model: "model-b".to_string(),
                price_in: Measurement::Observed(99),
                price_out: Measurement::Observed(99),
            }, // Differs -> included
            Registered {
                arm: "arm-3".to_string(),
                model: "model-unlisted".to_string(),
                price_in: Measurement::Observed(1),
                price_out: Measurement::Observed(1),
            }, // Unquoted -> omitted
            Registered {
                arm: "arm-4".to_string(),
                model: "model-c".to_string(),
                price_in: Measurement::Missing(Absent::NotAttempted),
                price_out: Measurement::Missing(Absent::NotAttempted),
            }, // Fills -> included
            Registered {
                arm: "arm-5".to_string(),
                model: "model-a".to_string(),
                price_in: Measurement::Observed(10),
                price_out: Measurement::Observed(20),
            }, // Agrees -> omitted
            Registered {
                arm: "arm-6".to_string(),
                model: "model-d".to_string(),
                price_in: Measurement::Observed(0),
                price_out: Measurement::Observed(0),
            }, // Differs -> included
        ];

        let result = pending(&regs, &cat);
        let expected = vec![
            (
                "arm-2".to_string(),
                Change::Differs {
                    price_in: 30,
                    price_out: 40,
                },
            ),
            (
                "arm-4".to_string(),
                Change::Fills {
                    price_in: 50,
                    price_out: 60,
                },
            ),
            (
                "arm-6".to_string(),
                Change::Differs {
                    price_in: 70,
                    price_out: 80,
                },
            ),
        ];
        assert_eq!(result, expected);
    }

    #[test]
    fn pending_preserves_duplicates_no_deduplication() {
        let cat = [Quoted {
            model: "model-x".to_string(),
            price_in: 10,
            price_out: 20,
        }];
        let regs = [
            Registered {
                arm: "dup-arm".to_string(),
                model: "model-x".to_string(),
                price_in: Measurement::Observed(1),
                price_out: Measurement::Observed(2),
            },
            Registered {
                arm: "dup-arm".to_string(),
                model: "model-x".to_string(),
                price_in: Measurement::Observed(3),
                price_out: Measurement::Observed(4),
            },
        ];

        let result = pending(&regs, &cat);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "dup-arm");
        assert_eq!(result[1].0, "dup-arm");
        assert_eq!(
            result[0].1,
            Change::Differs {
                price_in: 10,
                price_out: 20,
            }
        );
        assert_eq!(
            result[1].1,
            Change::Differs {
                price_in: 10,
                price_out: 20,
            }
        );
    }

    #[test]
    fn boundary_max_u64_price() {
        let cat = [Quoted {
            model: "model-max".to_string(),
            price_in: u64::MAX,
            price_out: u64::MAX,
        }];
        let reg = Registered {
            arm: "arm-max".to_string(),
            model: "model-max".to_string(),
            price_in: Measurement::Observed(u64::MAX),
            price_out: Measurement::Observed(u64::MAX),
        };
        assert_eq!(compare(&reg, &cat), Change::Agrees);

        let reg_missing = Registered {
            arm: "arm-max".to_string(),
            model: "model-max".to_string(),
            price_in: Measurement::Missing(Absent::NotAttempted),
            price_out: Measurement::Missing(Absent::NotAttempted),
        };
        assert_eq!(
            compare(&reg_missing, &cat),
            Change::Fills {
                price_in: u64::MAX,
                price_out: u64::MAX,
            }
        );
    }
}
