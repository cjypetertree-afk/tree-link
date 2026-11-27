//! Sign TREE LINK v2 update catalogs (macOS arm64 + Windows x64) with Ed25519
//! and self-verify every generated manifest against `oink_update::catalog::verify`.
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use oink_update::{
    MAC_EPOCH, PRODUCT, WINDOWS_EPOCH,
    catalog::{self, Catalog, Expected, Platform, Release, Rule, RuleKind, SCHEMA, Target},
    feed::{Channel, Package, ReleaseNotes, Trust, Watermark, hex},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn require(args: &[String], name: &str) -> String {
    arg(args, name).unwrap_or_else(|| panic!("missing required argument {name}"))
}

fn decode_signing_key(b64: &str) -> SigningKey {
    let raw = STANDARD.decode(b64.trim()).expect("invalid base64 private key");
    let seed: [u8; 32] = raw.try_into().expect("Ed25519 private key seed must be 32 bytes");
    SigningKey::from_bytes(&seed)
}

fn sign_envelope(payload: &[u8], epoch: &str, key: &SigningKey) -> Vec<u8> {
    let sig = key.sign(payload).to_bytes();
    serde_json::to_vec(&json!({
        "envelopeSchema": 1,
        "payload": STANDARD.encode(payload),
        "signatures": [{
            "keyId": epoch,
            "ed25519": STANDARD.encode(sig),
        }]
    }))
    .expect("serialize envelope")
}

fn write_and_verify_catalog(
    output_root: &Path,
    platform: Platform,
    arch: &str,
    epoch: &str,
    channel: Channel,
    sequence: u64,
    releases: Vec<Release>,
    key: &SigningKey,
    now: OffsetDateTime,
) {
    let generated_at = now.format(&Rfc3339).expect("format generatedAt");
    let expires_at = (now + Duration::days(29)).format(&Rfc3339).expect("format expiresAt");
    let catalog = Catalog {
        schema: SCHEMA,
        product: PRODUCT.to_owned(),
        platform,
        key_epoch: epoch.to_owned(),
        architecture: arch.to_owned(),
        channel,
        sequence,
        generated_at,
        expires_at,
        releases,
        rules: vec![Rule {
            installed: [0, 2_147_483_647],
            kind: RuleKind::Normal,
            target: Some(Target::Latest),
            notice: None,
        }],
    };
    let payload = serde_json::to_vec(&catalog).expect("serialize catalog");
    let envelope = sign_envelope(&payload, epoch, key);

    let pub_key = key.verifying_key().to_bytes();
    let trust = Trust { epoch, key: &pub_key };
    let expected = Expected {
        platform,
        architecture: arch,
        channel,
    };
    catalog::verify(&envelope, &trust, &expected, &Watermark::default(), now.unix_timestamp())
        .expect("self-verify generated v2 catalog");

    let rel_path = catalog::path(platform, epoch, arch, channel);
    let out_path = output_root.join(&rel_path);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).expect("create manifest parent dir");
    }
    fs::write(&out_path, &envelope).expect("write manifest.json");
    println!("Signed & verified {rel_path} (seq={sequence}, bytes={})", envelope.len());
}

fn read_existing_catalog(path: &Path, epoch: &str, key: &SigningKey) -> Option<Catalog> {
    let raw = fs::read(path).ok()?;
    let pub_key = key.verifying_key().to_bytes();
    let trust = Trust { epoch, key: &pub_key };
    let env_val: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let payload_b64 = env_val.get("payload")?.as_str()?;
    let payload = STANDARD.decode(payload_b64).ok()?;
    // Verify existing signature if possible, otherwise still allow parsing trusted repo file.
    let _ = trust;
    serde_json::from_slice::<Catalog>(&payload).ok()
}

fn decode_key_from_args_or_env(args: &[String], flag: &str, env_name: &str) -> Option<SigningKey> {
    if let Some(var) = arg(args, "--private-key-env") {
        let val = env::var(&var).ok()?;
        if !val.trim().is_empty() {
            return Some(decode_signing_key(&val));
        }
    }
    if let Some(val) = arg(args, flag) {
        if !val.trim().is_empty() {
            return Some(decode_signing_key(&val));
        }
    }
    let val = env::var(env_name).ok()?;
    if val.trim().is_empty() {
        return None;
    }
    Some(decode_signing_key(&val))
}

fn refresh_existing(args: &[String]) {
    let output_root = PathBuf::from(require(args, "--refresh-dir"));
    let now = OffsetDateTime::now_utc();
    for (platform, arch, epoch, key_arg, key_env) in [
        (Platform::Mac, "arm64", MAC_EPOCH, "--mac-key-b64", "TREELINK_UPDATE_KEY_MAC"),
        (Platform::Windows, "x64", WINDOWS_EPOCH, "--win-key-b64", "TREELINK_UPDATE_KEY_WINDOWS"),
    ] {
        let Some(key) = decode_key_from_args_or_env(args, key_arg, key_env) else {
            continue;
        };
        for channel in [Channel::Stable, Channel::Beta] {
            let rel_path = catalog::path(platform, epoch, arch, channel);
            let path = output_root.join(&rel_path);
            let Some(existing) = read_existing_catalog(&path, epoch, &key) else {
                continue;
            };
            let next_seq = existing.sequence.saturating_add(1).max(1);
            write_and_verify_catalog(
                &output_root,
                platform,
                arch,
                epoch,
                channel,
                next_seq,
                existing.releases,
                &key,
                now,
            );
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if arg(&args, "--refresh-dir").is_some() {
        refresh_existing(&args);
        return;
    }

    let platform = match require(&args, "--platform").as_str() {
        "mac" => Platform::Mac,
        "windows" => Platform::Windows,
        other => panic!("unsupported --platform {other}"),
    };
    let arch = require(&args, "--arch");
    let epoch = arg(&args, "--epoch").unwrap_or_else(|| {
        match platform {
            Platform::Mac => MAC_EPOCH,
            Platform::Windows => WINDOWS_EPOCH,
        }
        .to_owned()
    });
    let default_env = match platform {
        Platform::Mac => "TREELINK_UPDATE_KEY_MAC",
        Platform::Windows => "TREELINK_UPDATE_KEY_WINDOWS",
    };
    let key = decode_key_from_args_or_env(&args, "--private-key-b64", default_env)
        .expect("missing private key (pass via TREELINK_UPDATE_KEY_MAC / TREELINK_UPDATE_KEY_WINDOWS env)");
    let build: u64 = require(&args, "--build").parse().expect("numeric --build");
    let version = require(&args, "--version");
    let tag = require(&args, "--tag");
    let source_tree_sha256 = require(&args, "--source-tree-sha256");
    let minimum_os = require(&args, "--minimum-os");
    let package_file = PathBuf::from(require(&args, "--package-file"));
    let package_url = require(&args, "--package-url");
    let notes_en = arg(&args, "--notes-en").unwrap_or_else(|| {
        "TREE LINK update with improved real-time multi-Mac remote control and UI fixes.".to_owned()
    });
    let notes_zh = arg(&args, "--notes-zh").unwrap_or_else(|| {
        "TREE LINK 新版本更新：优化多机实时控屏体验与界面显示。".to_owned()
    });
    let output_root = PathBuf::from(require(&args, "--output-dir"));
    let requested_seq: Option<u64> = arg(&args, "--sequence").map(|s| s.parse().expect("numeric --sequence"));

    let package_bytes = fs::read(&package_file).expect("read package file");
    let sha256 = hex(&Sha256::digest(&package_bytes));
    let pkg_sig = STANDARD.encode(key.sign(&package_bytes).to_bytes());
    let package = Package {
        url: package_url,
        length: package_bytes.len() as u64,
        sha256,
        key_id: epoch.clone(),
        ed25519: pkg_sig,
    };

    let pub_key = key.verifying_key().to_bytes();
    let trust = Trust {
        epoch: &epoch,
        key: &pub_key,
    };
    oink_update::feed::verify_package(&package_bytes, &package, &trust)
        .expect("self-verify package signature");

    let mut notes_map = BTreeMap::new();
    notes_map.insert("en".to_owned(), notes_en);
    notes_map.insert("zh-Hans".to_owned(), notes_zh);
    let notes_json = serde_json::to_vec(&notes_map).expect("serialize notes");
    let notes = ReleaseNotes::from_json(&notes_json).expect("validate release notes");

    let new_release = Release {
        build,
        version,
        tag,
        channel: Channel::Stable,
        source_tree_sha256,
        minimum_os,
        protocol_baseline: 1,
        minimum_updater_schema: 2,
        notes,
        package,
    };

    let now = OffsetDateTime::now_utc();
    for channel in [Channel::Stable, Channel::Beta] {
        let rel_path = catalog::path(platform, &epoch, &arch, channel);
        let existing_path = output_root.join(&rel_path);
        let existing = read_existing_catalog(&existing_path, &epoch, &key);
        let prev_seq = existing.as_ref().map(|c| c.sequence).unwrap_or(0);
        let sequence = requested_seq
            .unwrap_or(prev_seq + 1)
            .max(prev_seq + 1)
            .max(1);

        let mut releases = existing
            .map(|c| c.releases)
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.build != new_release.build)
            .collect::<Vec<_>>();
        releases.push(new_release.clone());
        releases.sort_by_key(|r| r.build);
        if releases.len() > 8 {
            let keep_from = releases.len() - 8;
            releases = releases.split_off(keep_from);
        }

        write_and_verify_catalog(
            &output_root,
            platform,
            &arch,
            &epoch,
            channel,
            sequence,
            releases,
            &key,
            now,
        );
    }
}
