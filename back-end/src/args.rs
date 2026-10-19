///Command line arguments of the server version.
///
///usage: `back-end [vcd file] [listen address]`
pub struct Args {
    pub vcd_path: Option<String>,
    pub addr: Option<String>,
}

impl Args {
    pub fn new() -> Self {
        let mut args = std::env::args().skip(1);
        Self {
            vcd_path: args.next(),
            addr: args.next(),
        }
    }
}

impl Default for Args {
    fn default() -> Self {
        Self::new()
    }
}
