//! Downloads and parses a feed: `cargo run --example feed -- <url>`.
#[tokio::main]
async fn main() {
    for url in std::env::args().skip(1) {
        match hearthgate_firewall::feeds::download(&url).await {
            Ok(text) => {
                let nets = hearthgate_firewall::feeds::parse(&text);
                println!("{url}: {} networks, first {:?}", nets.len(), nets.first());
            }
            Err(e) => println!("{url}: {e}"),
        }
    }
}
