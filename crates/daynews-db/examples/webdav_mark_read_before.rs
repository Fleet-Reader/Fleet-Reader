// SPDX-License-Identifier: Apache-2.0
//! Offline maintenance through the regular journal. Stop the app and take a backup first.
//! Usage: webdav_mark_read_before DATABASE PUBLIC_CONFIG UNIX_CUTOFF
#[cfg(feature = "experimental-webdav")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use daynews_db::{
        Db,
        webdav::{self, day_sync::Identity},
    };
    use std::sync::Arc;
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: webdav_mark_read_before DATABASE PUBLIC_CONFIG UNIX_CUTOFF".into());
    }
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(&args[2])?)?;
    let identity: Identity = serde_json::from_value(config["identity"].clone())?;
    let cutoff: i64 = args[3].parse()?;
    let count = futures_lite::future::block_on(async {
        let replica = webdav::open(&args[1], Arc::new(identity), Arc::new(|_| Ok(()))).await?;
        let result = replica
            .worker()
            .write(move |c| {
                Db {
                    container: c.clone(),
                }
                .set_read_before(cutoff)
            })
            .await;
        replica.close().await?;
        Ok::<_, Box<dyn std::error::Error>>(result?)
    })?;
    println!("Marked {count} articles read; changes are journaled for synchronization.");
    Ok(())
}
#[cfg(not(feature = "experimental-webdav"))]
fn main() {
    panic!("requires experimental-webdav");
}
