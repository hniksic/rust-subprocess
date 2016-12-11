extern crate libc;

pub mod popen;

#[cfg(test)]
mod tests {
    use popen::Popen;

    #[test]
    fn new() {
        let p = Popen::create(&["ls", "-al"]).unwrap();
        println!("{:?}", p);
    }
}
