//! Embeds the dashboard only when it has been built (`npm run build` in
//! `dashboard/`), so the Rust code builds without Node installed.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(dashboard)");
    let dist = std::path::Path::new("../../dashboard/dist");
    println!("cargo::rerun-if-changed=../../dashboard/dist");
    if dist.join("index.html").is_file() {
        println!("cargo::rustc-cfg=dashboard");
    }
}
