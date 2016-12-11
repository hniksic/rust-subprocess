extern crate libc;

pub mod popen;

#[cfg(test)]
mod tests {
    use popen::Popen;

    #[test]
    fn good_cmd() {
        Popen::create(&["ls", "-al"]).unwrap();
    }

    #[test]
    fn bad_cmd() {
        let result = Popen::create(&["nosuchcommand"]);
        assert!(result.is_err());
    }
}
