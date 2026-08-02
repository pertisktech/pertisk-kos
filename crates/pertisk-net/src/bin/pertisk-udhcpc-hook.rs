//! Shell-less BusyBox `udhcpc` lease script for production images.
//!
//! Installed as `/usr/lib/pertisk/udhcpc-hook` and invoked via `udhcpc -s`.

fn main() {
    #[cfg(target_os = "linux")]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Err(err) = pertisk_net::udhcpc_hook::run_from_env(&args) {
            eprintln!("pertisk-udhcpc-hook: {err}");
            std::process::exit(1);
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        eprintln!("pertisk-udhcpc-hook: Linux only");
        std::process::exit(1);
    }
}
