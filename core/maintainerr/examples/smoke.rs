// SPDX-License-Identifier: AGPL-3.0-or-later
#![allow(clippy::print_stdout)] // CLI example: printing is the point.
//! Read-only smoke test against a real Maintainerr: `cargo run -p maintainerr --example smoke -- <url>`
use maintainerr::Maintainerr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::args()
        .nth(1)
        .ok_or("usage: smoke <maintainerr url>")?;
    let m = Maintainerr::new(
        net::api_client("alwywatch-smoke")?,
        &url,
        std::env::var("MAINTAINERR_API_KEY").ok(),
    );
    println!(
        "version {} / media server {}",
        m.version().await?,
        m.media_server_type().await?
    );
    for c in m.collections().await? {
        let items = m.collection_items(c.id).await?;
        let dates: Vec<i64> = items.iter().filter_map(|i| i.deletion_date(&c)).collect();
        println!(
            "collection {} '{}' active={} type={} items={} (mediaCount {}) deletion range {:?}..{:?}",
            c.id,
            c.title,
            c.is_active,
            c.kind,
            items.len(),
            c.media_count,
            dates.iter().min(),
            dates.iter().max()
        );
        if let Some(first) = items.first() {
            let meta = m.meta(&first.media_server_id).await?;
            println!(
                "  first: {} ({:?}) tmdb={:?} tvdb={:?} poster={:?}",
                meta.title,
                meta.year,
                meta.tmdb_id,
                meta.tvdb_id,
                first.poster_url()
            );
        }
    }
    Ok(())
}
