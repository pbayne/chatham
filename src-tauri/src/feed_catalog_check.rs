//! Health check for the Feeds Discover catalog (src/content/feedCatalog.json):
//! fetch every suggested feed and parse it with Chatham's own parser, so a
//! dead or moved feed is caught before a user subscribes to it. Network-bound,
//! so ignored by default:
//!   cargo test --lib feed_catalog_is_healthy -- --ignored --nocapture
#[cfg(test)]
mod tests {
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    #[ignore]
    async fn feed_catalog_is_healthy() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/content/feedCatalog.json");
        let catalog: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let client = reqwest::Client::builder()
            .user_agent(concat!("Chatham/", env!("CARGO_PKG_VERSION"), " (+https://github.com/pbayne/chatham)"))
            .timeout(std::time::Duration::from_secs(25))
            .build()
            .unwrap();
        let mut tasks = Vec::new();
        for feed in catalog["categories"].as_array().unwrap().iter().flat_map(|c| c["feeds"].as_array().unwrap().clone()) {
            let client = client.clone();
            tasks.push(tokio::spawn(async move {
                let url = feed["url"].as_str().unwrap().to_string();
                let r = async {
                    let resp = client.get(&url).send().await.map_err(|e| format!("network: {e}"))?;
                    if !resp.status().is_success() {
                        return Err(format!("HTTP {}", resp.status()));
                    }
                    let fin = resp.url().to_string();
                    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
                    let parsed = crate::feeds::parse(&bytes, &fin).map_err(|e| e.to_string())?;
                    if parsed.items.is_empty() {
                        return Err("no items".into());
                    }
                    Ok(parsed.items.len())
                }
                .await;
                (feed["name"].as_str().unwrap().to_string(), url, r)
            }));
        }
        let mut failed = Vec::new();
        let total = tasks.len();
        for t in tasks {
            let (name, url, r) = t.await.unwrap();
            match r {
                Ok(n) => println!("ok   {name:28} {n:>5} items"),
                Err(e) => {
                    println!("FAIL {name:28} {e}  {url}");
                    failed.push(name);
                }
            }
        }
        assert!(failed.is_empty(), "{}/{} catalog feeds failed: {failed:?}", failed.len(), total);
    }
}
