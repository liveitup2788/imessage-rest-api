use std::io::Cursor;
use std::ops::Deref;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use keystore::software::{SoftwareEncryptor, SoftwareKeystore};
use keystore::{
    init_keystore, AesKeystoreKey, EncryptMode,
    KeystoreAccessRules, KeystoreDigest, KeystoreEncryptKey, KeystorePadding, RsaKey,
};
use log::info;
use plist::{Data, Dictionary, Value};
use rustpush::{
    default_provider, APSConnection, APSConnectionResource, APSMessage, APSState,
    ArcAnisetteClient, DefaultAnisetteProvider, IDSNGMIdentity, IDSUser, IMClient,
    LoginClientInfo, MADRID_SERVICE,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use tokio::sync::broadcast;

use rustpush::macos::MacOSConfig;
use rustpush::RelayConfig;
use rustpush::OSConfig;

// --- Types copied/adapted from api.rs ---

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum JoinedOSConfig {
    MacOS(Arc<MacOSConfig>),
    Relay(Arc<RelayConfig>),
}

impl JoinedOSConfig {
    pub fn config(&self) -> Arc<dyn OSConfig> {
        match self {
            Self::MacOS(conf) => conf.clone(),
            Self::Relay(conf) => conf.clone(),
        }
    }
}

impl Deref for JoinedOSConfig {
    type Target = dyn OSConfig;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::MacOS(conf) => conf.as_ref(),
            Self::Relay(conf) => conf.as_ref(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SavedHardwareState {
    pub push: APSState,
    #[serde(
        serialize_with = "bin_serialize",
        deserialize_with = "bin_deserialize"
    )]
    pub identity: Vec<u8>,
    pub os_config: JoinedOSConfig,
}

pub fn bin_serialize<S>(x: &[u8], s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_bytes(x)
}

pub fn bin_deserialize<'de, D>(d: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Data = Deserialize::deserialize(d)?;
    Ok(s.into())
}

fn bin_deserialize_16<'de, D>(d: D) -> Result<[u8; 16], D::Error>
where
    D: Deserializer<'de>,
{
    let s: Data = Deserialize::deserialize(d)?;
    let s: Vec<u8> = s.into();
    Ok(s.try_into().unwrap())
}

#[derive(Serialize, Deserialize)]
pub struct AnisetteState {
    #[serde(
        serialize_with = "bin_serialize",
        deserialize_with = "bin_deserialize_16"
    )]
    keychain_identifier: [u8; 16],
    provisioned: Option<ProvisionedAnisette>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ProvisionedAnisette {
    client_secret: Data,
    mid: Data,
    metadata: Data,
    rinfo: String,
    #[serde(default)]
    flavor: ProvisionedFlavor,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub enum ProvisionedFlavor {
    #[default]
    Mac,
    IOS,
}

#[derive(Serialize, Deserialize)]
struct GSAConfig {
    username: String,
    encrypted_password: Data,
    postdata_done: Option<bool>,
}

impl GSAConfig {
    fn get_password(&self) -> Result<Vec<u8>, rustpush::PushError> {
        let key = AesKeystoreKey::ensure(
            "gsa:password",
            256,
            KeystoreAccessRules {
                block_modes: vec![EncryptMode::Gcm],
                can_encrypt: true,
                can_decrypt: true,
                ..Default::default()
            },
        )?;
        let encoded = key.decrypt(self.encrypted_password.as_ref(), &EncryptMode::Gcm)?;
        Ok(encoded)
    }

}

// --- Helper functions ---

fn plist_to_string<T: serde::Serialize>(value: &T) -> Result<String, plist::Error> {
    let mut buf: Vec<u8> = Vec::new();
    let writer = Cursor::new(&mut buf);
    plist::to_writer_xml(writer, &value)?;
    Ok(String::from_utf8(buf).unwrap())
}

pub fn migrate(path: &str) -> bool {
    let dir = PathBuf::from_str(path).unwrap();
    let hw_config_path = dir.join("hw_info.plist");

    if let Ok(mut item) = plist::from_file::<_, Dictionary>(&hw_config_path) {
        if let Some(v) = item.get("os_config") {
            let config: JoinedOSConfig = plist::from_value(v).expect("got os ");
            if let Some(Value::Dictionary(dict)) = item.get_mut("push") {
                if let Some(Value::Dictionary(item)) = dict.get_mut("keypair") {
                    if let Some(private) = item.get_mut("private") {
                        if let Value::Data(cert) = private {
                            let handle = format!("activation:{}", config.get_serial_number());
                            RsaKey::import(
                                &handle,
                                1024,
                                cert,
                                KeystoreAccessRules {
                                    signature_padding: vec![KeystorePadding::PKCS1],
                                    digests: vec![KeystoreDigest::Sha1],
                                    can_sign: true,
                                    ..Default::default()
                                },
                            )
                            .expect("failed to import RSA");
                            *private = Value::String(handle);
                            plist::to_file_xml(&hw_config_path, &item)
                                .expect("failed to save!");
                        }
                    }
                }
            }
        }
        if let Some(value) = item.get_mut("identity") {
            if value.as_dictionary().is_some() {
                let identity: IDSNGMIdentity =
                    plist::from_value(value).expect("NGM Identity parse");
                *value = Value::Data(identity.save("openbubbles").expect("Failed to save"));
                plist::to_file_xml(&hw_config_path, &item).expect("failed to save!");
            }
        }
    }

    let id_path = dir.join("id.plist");
    if let Ok(mut users) = plist::from_file::<_, Vec<Dictionary>>(&id_path) {
        let mut modified = false;
        for user in &mut users {
            let user_id = user
                .get("user_id")
                .unwrap()
                .as_string()
                .unwrap()
                .to_string();
            if let Some(Value::Dictionary(item)) = user.get_mut("auth_keypair") {
                if let Some(private) = item.get_mut("private") {
                    if let Value::Data(cert) = private {
                        let handle = format!("ids:{user_id}");
                        RsaKey::import(
                            &handle,
                            2048,
                            cert,
                            KeystoreAccessRules {
                                signature_padding: vec![KeystorePadding::PKCS1],
                                digests: vec![KeystoreDigest::Sha1],
                                can_sign: true,
                                ..Default::default()
                            },
                        )
                        .expect("failed to import RSA");
                        *private = Value::String(handle);
                        modified = true;
                    }
                }
            }
            if let Some(Value::Dictionary(item)) = user.get_mut("registration") {
                for service in item.values_mut() {
                    if let Some(Value::Dictionary(item)) = service
                        .as_dictionary_mut()
                        .unwrap()
                        .get_mut("id_keypair")
                    {
                        if let Some(private) = item.get_mut("private") {
                            if let Value::Data(_cert) = private {
                                let handle = format!("ids:{user_id}");
                                *private = Value::String(handle);
                            }
                        }
                    }
                }
            }
        }
        if modified {
            plist::to_file_xml(&id_path, &users).expect("failed to save!");
        }
    }

    // Skip cloudkit/keychain migration - not needed for sending messages
    // Skip GSA password migration - already done by Flatpak app

    false
}

pub fn read_hardware(path: &str) -> Option<SavedHardwareState> {
    let dir = PathBuf::from_str(path).unwrap();
    let hw_config_path = dir.join("hw_info.plist");
    plist::from_file::<_, SavedHardwareState>(&hw_config_path).ok()
}

pub fn restore_users(path: &str) -> Option<Vec<IDSUser>> {
    let dir = PathBuf::from_str(path).unwrap();
    let id_path = dir.join("id.plist");
    plist::from_file::<_, Vec<IDSUser>>(&id_path).ok()
}

async fn get_login_config(
    conf_dir: &PathBuf,
    conf: &JoinedOSConfig,
    conn: &APSConnection,
) -> LoginClientInfo {
    let anisette_dir = conf_dir.join("anisette_test");
    let config_path = anisette_dir.join("state.plist");

    let require_mac =
        if let Ok(decoded) = plist::from_file::<_, AnisetteState>(config_path) {
            matches!(
                decoded.provisioned,
                Some(ProvisionedAnisette {
                    flavor: ProvisionedFlavor::Mac,
                    ..
                })
            )
        } else {
            false
        };

    conf.get_gsa_config(&*conn.state.read().await, require_mac)
}

pub async fn make_anisette(
    path: &str,
    config: &JoinedOSConfig,
    conn: &APSConnection,
) -> ArcAnisetteClient<DefaultAnisetteProvider> {
    let dir = PathBuf::from_str(path).unwrap();
    default_provider(
        get_login_config(&dir, config, conn).await,
        dir.join("anisette_test"),
    )
}

pub async fn make_imclient(
    path: &str,
    conn: &APSConnection,
    users: &Vec<IDSUser>,
    identity: &IDSNGMIdentity,
) -> Arc<IMClient> {
    let dir = PathBuf::from_str(path).unwrap();
    let id_path = dir.join("id.plist");

    // Create incident marker if needed (same as original)
    let incident_path = dir.join("incident");
    if !incident_path.exists() {
        if plist::from_file::<_, rustpush::KeyCache>(dir.join("id_cache.plist")).is_ok() {
            let _ = std::fs::File::create(dir.join("incident_affected"));
        }
        let _ = std::fs::File::create(incident_path);
    }

    Arc::new(
        IMClient::new(
            conn.clone(),
            users.clone(),
            identity.clone(),
            &[&MADRID_SERVICE],
            dir.join("id_cache.plist"),
            conn.os_config.clone(),
            Box::new(move |updated_keys| {
                info!("updated keys");
                std::fs::write(&id_path, plist_to_string(&updated_keys).unwrap()).unwrap();
            }),
        )
        .await,
    )
}

pub async fn setup_push(
    config: &JoinedOSConfig,
    identity: &IDSNGMIdentity,
    state: Option<APSState>,
    state_path: &str,
) -> (APSConnection, Option<rustpush::PushError>) {
    let state_path = PathBuf::from_str(state_path)
        .unwrap()
        .join("hw_info.plist");
    let (conn, error) = APSConnectionResource::new(config.config(), state).await;

    let saved_identity = identity.save("openbubbles").expect("failed to save");
    if error.is_none() {
        let state = SavedHardwareState {
            push: conn.state.read().await.clone(),
            os_config: config.clone(),
            identity: saved_identity.clone().into(),
        };
        std::fs::write(&state_path, plist_to_string(&state).unwrap()).unwrap();
    }

    let mut to_refresh = conn.generated_signal.subscribe();
    let reconn_conn = Arc::downgrade(&conn);
    let config_ref = config.clone();
    tokio::spawn(async move {
        loop {
            match to_refresh.recv().await {
                Ok(()) => {
                    let Some(conn) = reconn_conn.upgrade() else {
                        break;
                    };
                    let state = SavedHardwareState {
                        push: conn.state.read().await.clone(),
                        os_config: config_ref.clone(),
                        identity: saved_identity.clone().into(),
                    };
                    std::fs::write(&state_path, plist_to_string(&state).unwrap()).unwrap();
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    (conn, error)
}

pub async fn restore_account(
    path: &str,
    anisette: &ArcAnisetteClient<DefaultAnisetteProvider>,
    config: &JoinedOSConfig,
    conn: &APSConnection,
) -> Option<()> {
    let dir = PathBuf::from_str(path).unwrap();

    let mut state = plist::from_file::<_, GSAConfig>(&dir.join("gsa.plist")).ok()?;

    let mut apple_account = rustpush::AppleAccount::new_with_anisette(
        get_login_config(&dir, config, conn).await,
        anisette.clone(),
    )
    .expect("failed to create apple account");

    apple_account.username = Some(state.username.clone());
    apple_account.hashed_password = state.get_password().ok();

    if state.postdata_done.is_none() {
        info!("Updating postdata");
        let _ = apple_account
            .update_postdata("Apple Device", None, &["icloud", "imessage", "facetime"])
            .await;
        state.postdata_done = Some(true);
        plist::to_file_xml(dir.join("gsa.plist"), &state).unwrap();
    }

    Some(())
}

/// Restore the full session from Flatpak data directory.
/// Returns (IMClient, APSConnection, sender_handle).
pub async fn restore(
    path: &str,
) -> anyhow::Result<(Arc<IMClient>, APSConnection, broadcast::Receiver<APSMessage>)> {
    let dir = PathBuf::from_str(path).unwrap();
    let keystore_path = dir.join("keystore.plist");

    init_keystore(SoftwareKeystore {
        state: plist::from_file(&keystore_path).unwrap_or_default(),
        update_state: Box::new(move |state| {
            plist::to_file_xml(&keystore_path, state).unwrap();
        }),
        encryptor: SoftwareEncryptor(*b"desktopisinsecureyoushouldn'tber"),
    });

    if let Err(err) = std::panic::catch_unwind(|| {
        migrate(path);
    }) {
        if let Some(s) = err.downcast_ref::<&str>() {
            log::error!("Migration panic: {}", s);
        } else if let Some(s) = err.downcast_ref::<String>() {
            log::error!("Migration panic: {}", s);
        }
        anyhow::bail!("Migration panicked");
    }

    let hardware = read_hardware(path).ok_or_else(|| anyhow::anyhow!("No hw_info.plist found"))?;
    let users = restore_users(path).ok_or_else(|| anyhow::anyhow!("No id.plist found"))?;
    let config = &hardware.os_config;
    let identity = IDSNGMIdentity::restore(hardware.identity.as_ref(), "openbubbles")?;

    info!("Setting up APS connection...");
    let (conn, push_err) =
        setup_push(config, &identity, Some(hardware.push.clone()), path).await;
    if let Some(err) = push_err {
        log::warn!("Push setup warning: {}", err);
    }

    info!("Creating IMClient...");
    let client = make_imclient(path, &conn, &users, &identity).await;

    info!("Setting up anisette...");
    let anisette = make_anisette(path, config, &conn).await;

    info!("Restoring account...");
    let _ = restore_account(path, &anisette, config, &conn).await;

    let aps_receiver = conn.messages_cont.subscribe();

    info!("Session restored successfully");
    Ok((client, conn, aps_receiver))
}
