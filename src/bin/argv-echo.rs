// Test helper that echoes a slice of its argv.  With no extra arguments it
// prints argv[0] (useful for verifying argv[0] overrides); with one or more
// arguments it prints argv[1] (useful for verifying arg escaping/passing).

fn main() {
    let mut args = std::env::args();
    let arg0 = args.next().unwrap_or_default();
    match args.next() {
        Some(arg) => print!("{arg}"),
        None => print!("{arg0}"),
    }
}
