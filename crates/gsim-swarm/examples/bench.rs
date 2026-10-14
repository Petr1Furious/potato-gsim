//! `cargo run --release -p gsim-swarm --example bench [scenario] [count...]`
use gsim_swarm::scenario::Params;
use gsim_swarm::{bench, Level};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().map(String::as_str).unwrap_or("galaxy");
    println!("kernels: {}, {} threads", Level::detect().name(), rayon::current_num_threads());
    let counts: Vec<usize> = args.iter().skip(1).filter_map(|a| a.parse().ok()).collect();
    for count in counts {
        println!("{name} {count:>9} bodies: {:6.2} ms per step", bench::step_ms(name, &Params::new(), count).unwrap());
    }
    println!("{name}: about {} bodies fit a {} ms step", bench::suggest(name, &Params::new(), bench::STEP_BUDGET_MS).unwrap(), bench::STEP_BUDGET_MS);
}
