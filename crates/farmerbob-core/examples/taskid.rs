fn main() {
    for p in std::env::args().skip(1) {
        match std::fs::read_to_string(&p) {
            Ok(t) => println!("  {}  {}", farmerbob_core::taskid::task_id(&t), p),
            Err(e) => eprintln!("  {p}: {e}"),
        }
    }
}
