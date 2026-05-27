// TODO(ai-review): review for style and correctness
//! One-off scratch: pick a single FQN, parse both Assembly-CSharp.dll
//! sides, dump the `TypeDefinition` Debug output for each, and diff
//! the two side-by-side. Lets us see exactly which dotnetdll field
//! makes the Hash diverge for a "false positive" type — one where
//! ilspy renders identical C# but dll-diff says Changed.

use std::sync::Arc;

use anyhow::Result;
use dll_diff::dotnetdll::prelude::{ReadOptions, Resolution};
use steam_depot_vfs::DepotStore;
use steam_depot_vfs::session::LazyCachedAuth;
use steam_multiversion_viewer::config::Config;

const APP_ID: u32 = 367520;
const DEPOT_ID: u32 = 367523;
const FROM_MANIFEST: u64 = 5829533265112705522;
const TO_MANIFEST: u64 = 708613018541602983;
const BRANCH: &str = "public";
const DLL_PATH: &str = "hollow_knight_Data/Managed/Assembly-CSharp.dll";

const FQN: &str = "InControl.TouchInputDevice";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let auth = LazyCachedAuth::prepare(
        LazyCachedAuth::default_refresh_token_cache(),
        std::env::var("STEAM_USERNAME").expect("missing STEAM_USERNAME"),
        std::env::var("STEAM_PASSWORD").expect("missing STEAM_PASSWORD"),
    )
    .await?;
    let auth = Arc::new(auth);

    let config = Config::load_or_default()?;
    let store = DepotStore::new(config.store_root.as_std_path().to_path_buf());
    let from_manifest = store
        .open_depot_manifest(auth.clone(), APP_ID, DEPOT_ID, FROM_MANIFEST, BRANCH)
        .await?;
    let to_manifest = store
        .open_depot_manifest(auth, APP_ID, DEPOT_ID, TO_MANIFEST, BRANCH)
        .await?;
    let (from_bytes, to_bytes) = tokio::try_join!(
        from_manifest.read_full(DLL_PATH),
        to_manifest.read_full(DLL_PATH)
    )?;
    let from_bytes = from_bytes.to_vec();
    let to_bytes = to_bytes.to_vec();

    let (from_dump, to_dump) = tokio::task::spawn_blocking(move || -> Result<(String, String)> {
        let from_res = Resolution::parse(&from_bytes, ReadOptions::default())?;
        let to_res = Resolution::parse(&to_bytes, ReadOptions::default())?;
        Ok((dump_type(&from_res, FQN)?, dump_type(&to_res, FQN)?))
    })
    .await??;

    let unified = similar::TextDiff::from_lines(&from_dump, &to_dump)
        .unified_diff()
        .context_radius(2)
        .header("from", "to")
        .to_string();
    println!("{unified}");
    Ok(())
}

fn dump_type(res: &Resolution<'_>, fqn: &str) -> Result<String> {
    for (idx, td) in res.enumerate_type_definitions() {
        let name = match &td.namespace {
            Some(ns) if !ns.is_empty() => format!("{}.{}", ns, td.name),
            _ => td.name.to_string(),
        };
        if name == fqn {
            return Ok(format!("{:#?}", &res[idx]));
        }
    }
    anyhow::bail!("type {fqn} not found")
}
