//! JNI is only an adapter; all durable behavior lives in zork-client-core.
#[cfg(all(target_os = "android", debug_assertions))]
mod diagnostics;
#[cfg(all(target_os = "android", debug_assertions))]
mod local_script_fixture;
mod observations;
use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};
use zork_client_core::{Client, Command, LocalClient};

struct Host {
    root: PathBuf,
    executor: tokio::runtime::Runtime,
    client: tokio::sync::Mutex<Client>,
    local: LocalClient,
    observations: observations::Registry,
}

static HOST: OnceLock<Mutex<Option<Arc<Host>>>> = OnceLock::new();

fn host(root: &Path) -> Result<Arc<Host>> {
    let mut host = HOST
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| anyhow::anyhow!("客户端需要重新启动"))?;
    if host.is_none() {
        let executor = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("zork-mobile")
            .enable_all()
            .build()?;
        let client = Client::open(root)?;
        #[cfg(all(target_os = "android", debug_assertions))]
        diagnostics::init(root);
        *host = Some(Arc::new(Host {
            root: root.to_owned(),
            executor,
            local: client.local(),
            client: tokio::sync::Mutex::new(client),
            observations: observations::Registry::default(),
        }));
    }
    let host = host.as_ref().expect("initialized host");
    ensure!(host.root == root, "cannot switch client identity directory");
    Ok(host.clone())
}

pub fn call(root: &Path, request: &str) -> Result<Value> {
    ensure!(request.len() <= 128 * 1024, "request too large");
    let command: Command = serde_json::from_str(request)?;
    let host = host(root)?;
    if command.is_local() {
        let _runtime = host.executor.enter();
        host.local.execute(command)
    } else {
        // Java callers have small stacks. Construct and poll the business
        // future on a Rust worker; the JNI thread only waits for its result.
        let owner = host.clone();
        let work = host.executor.spawn_blocking(move || {
            owner
                .executor
                .block_on(async { Box::pin(owner.client.lock().await.execute(command)).await })
        });
        host.executor.block_on(work)?
    }
}

pub fn response(root: &Path, request: &str) -> String {
    let started = std::time::Instant::now();
    let result = call(root, request);
    #[cfg(all(target_os = "android", debug_assertions))]
    diagnostics::record_response(request, &result, started.elapsed());
    #[cfg(not(all(target_os = "android", debug_assertions)))]
    let _ = started;
    match result {
        Ok(data) => json!({"ok":true,"data":data}).to_string(),
        Err(error) => json!({"ok":false,"error":error.to_string()}).to_string(),
    }
}

pub fn observation(root: &Path, request: &str) -> String {
    let result = (|| -> Result<Value> {
        ensure!(request.len() <= 4096, "observation request too large");
        let host = host(root)?;
        host.observations
            .execute(&host, serde_json::from_str(request)?)
    })();
    match result {
        Ok(data) => json!({"ok":true,"data":data}).to_string(),
        Err(error) => json!({"ok":false,"error":error.to_string()}).to_string(),
    }
}

/// Pure input projection; it does not open a client, acquire IO locks or request a network.
pub fn validate_model(input: &str, models: &str) -> Result<Value> {
    let input: zork_client_core::model_edit::ModelInput = serde_json::from_str(input)?;
    let models: Vec<Value> = serde_json::from_str(models)?;
    Ok(serde_json::to_value(input.errors(&models))?)
}

#[cfg(debug_assertions)]
pub fn preview_models(input: &str, models: &str) -> Result<Value> {
    let input: zork_client_core::model_edit::ModelInput = serde_json::from_str(input)?;
    let models: Vec<Value> = serde_json::from_str(models)?;
    Ok(json!({"models":input.apply(models)?}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zork_client_core::store::{ClientStore, SavedNode};

    #[test]
    fn local_drafts_do_not_wait_for_an_in_flight_network_command() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        store
            .save_node(&SavedNode {
                id: "test-peer".into(),
                name: "fixture".into(),
                url: String::new(),
                token: None,
                local: false,
                mesh: None,
                group: None,
            })
            .unwrap();
        let host = host(root.path()).unwrap();
        // Network operations hold this lock across await. A draft must commit
        // through the independent local handle even while that lock is held.
        let _in_flight_request = host.client.blocking_lock();
        let path = root.path().to_owned();
        let (sent, received) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            sent.send(call(
                &path,
                &json!({"op":"draft","peer":"test-peer",
                "session":"chat","content":"断网仍能保存 👋"})
                .to_string(),
            ))
            .unwrap();
        });
        received
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("draft blocked behind network transport")
            .unwrap();
        writer.join().unwrap();
        let path = root.path().to_owned();
        let (closed, completion) = std::sync::mpsc::channel();
        let closer = std::thread::spawn(move || {
            closed
                .send(call(
                    &path,
                    &json!({"op":"close_service","view_id":"dismissed-preview"}).to_string(),
                ))
                .unwrap();
        });
        completion
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("service close blocked behind network transport")
            .unwrap();
        closer.join().unwrap();
        store
            .put(
                "test-peer",
                "public-settings",
                &json!({"ready":true,"profiles":[{"profile_id":"offline"}]}),
            )
            .unwrap();
        let path = root.path().to_owned();
        let (sent, received) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            sent.send(call(
                &path,
                &json!({"op":"settings","peer":"test-peer","cached_only":true}).to_string(),
            ))
            .unwrap();
        });
        let settings = received
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("settings blocked behind network transport")
            .unwrap();
        assert_eq!(settings["profiles"][0]["profile_id"], "offline");
        reader.join().unwrap();
        assert_eq!(
            store
                .get::<String>("test-peer", "draft:chat")
                .unwrap()
                .as_deref(),
            Some("断网仍能保存 👋")
        );
        drop(_in_flight_request);
        let path = root.path().to_owned();
        let small_stack = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || call(&path, r#"{"op":"diagnose_connections"}"#))
            .unwrap();
        let result = small_stack.join().unwrap().unwrap();
        assert_eq!(result["items"][0]["reachable"], false);
    }
}

#[cfg(target_os = "android")]
mod liquid;

#[cfg(target_os = "android")]
mod android {
    use jni::{
        jni_sig, jni_str,
        objects::{JObject, JString},
        EnvUnowned, JValue,
    };

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_watch<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        root: JString<'a>,
        handle: jni::sys::jlong,
        generation: jni::sys::jlong,
        listener: JObject<'a>,
    ) -> JString<'a> {
        env.with_env(|env| {
            let result = (|| -> anyhow::Result<()> {
                anyhow::ensure!(handle > 0 && generation > 0, "invalid observation handle");
                let vm = env.get_java_vm()?;
                let listener = env.new_global_ref(listener)?;
                let host = super::host(std::path::Path::new(&root.to_string()))?;
                host.observations.watch(
                    &host.executor,
                    handle as u64,
                    generation as u64,
                    move |urgent, closed| {
                        let result: jni::errors::Result<()> = vm.attach_current_thread(|env| {
                            env.call_method(
                                &listener,
                                jni_str!("onReady"),
                                jni_sig!("(JJZZ)V"),
                                &[
                                    JValue::Long(handle),
                                    JValue::Long(generation),
                                    JValue::Bool(urgent),
                                    JValue::Bool(closed),
                                ],
                            )?;
                            Ok(())
                        });
                        result.is_ok()
                    },
                )
            })();
            let result = match result {
                Ok(()) => serde_json::json!({"ok":true}),
                Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}),
            };
            JString::from_str(env, result.to_string())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_observe<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        root: JString<'a>,
        request: JString<'a>,
    ) -> JString<'a> {
        env.with_env(|env| {
            JString::from_str(
                env,
                super::observation(
                    std::path::Path::new(&root.to_string()),
                    &request.to_string(),
                ),
            )
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_textAttachmentLimit<'a>(
        _env: EnvUnowned<'a>,
        _this: JObject<'a>,
    ) -> jni::sys::jint {
        zork_client_core::state::TEXT_ATTACHMENT_LIMIT as jni::sys::jint
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_newId<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
    ) -> JString<'a> {
        env.with_env(|env| JString::from_str(env, ulid::Ulid::new().to_string()))
            .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_composerState<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        input: JString<'a>,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let state = serde_json::from_str::<zork_client_core::composer::QueuedComposer>(
                &input.to_string(),
            )
            .map(|input| input.state())
            .unwrap_or_default();
            JString::from_str(env, serde_json::to_string(&state).unwrap())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_validateModel<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        input: JString<'a>,
        models: JString<'a>,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let value = super::validate_model(&input.to_string(), &models.to_string())
                .unwrap_or_else(
                    |_| serde_json::json!([{"field":"profile-model","message":"模型输入不可用"}]),
                );
            JString::from_str(env, value.to_string())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[cfg(debug_assertions)]
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_LocalScriptFixtureBridge_seed<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        root: JString<'a>,
        input: JString<'a>,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let value = match super::local_script_fixture::seed(
                std::path::Path::new(&root.to_string()),
                &input.to_string(),
            ) {
                Ok(value) => serde_json::json!({"ok":true,"data":value}),
                Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}),
            };
            JString::from_str(env, value.to_string())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[cfg(debug_assertions)]
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_previewModels<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        input: JString<'a>,
        models: JString<'a>,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let value = match super::preview_models(&input.to_string(), &models.to_string()) {
                Ok(value) => serde_json::json!({"ok":true,"data":value}),
                Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}),
            };
            JString::from_str(env, value.to_string())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_isLocal<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        request: JString<'a>,
    ) -> jni::sys::jboolean {
        env.with_env(|_| -> Result<_, jni::errors::Error> {
            Ok(
                serde_json::from_str::<zork_client_core::Command>(&request.to_string())
                    .is_ok_and(|c| c.is_local()),
            )
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_agentChoices<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        profiles: JString<'a>,
        profile: JString<'a>,
        model: JString<'a>,
        thinking: JString<'a>,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let profiles = serde_json::from_str::<Vec<zork_client_core::api::ProfileInfo>>(
                &profiles.to_string(),
            )
            .unwrap_or_default();
            let value = zork_client_core::agent_edit::choices(
                &profiles,
                &profile.to_string(),
                &model.to_string(),
                &thinking.to_string(),
            );
            JString::from_str(env, value.to_string())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_connectionChoices<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        catalog: JString<'a>,
        subscription: jni::sys::jboolean,
        provider: JString<'a>,
        billing: JString<'a>,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let catalog = serde_json::from_str::<Vec<serde_json::Value>>(&catalog.to_string())
                .unwrap_or_default();
            let value = zork_client_core::model_edit::connection_choices(
                &catalog,
                subscription,
                &provider.to_string(),
                &billing.to_string(),
            );
            JString::from_str(env, value.to_string())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_modelForm<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        query: JString<'a>,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let value: serde_json::Value =
                serde_json::from_str(&query.to_string()).unwrap_or_default();
            let form = if let (Some(input), Some(copy)) = (value.get("input"), value.get("copy")) {
                zork_client_core::model_edit::copy_form(
                    serde_json::from_value(input.clone()).unwrap_or_default(),
                    copy.clone(),
                )
            } else {
                zork_client_core::model_edit::model_form(
                    &value["profile"],
                    value["providers"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    value.get("model").filter(|m| !m.is_null()).cloned(),
                )
            };
            JString::from_str(env, serde_json::to_string(&form).unwrap())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_copyableModel<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        model: JString<'a>,
    ) -> jni::sys::jboolean {
        env.with_env(|_| -> Result<_, jni::errors::Error> {
            let value: serde_json::Value =
                serde_json::from_str(&model.to_string()).unwrap_or_default();
            Ok(zork_client_core::model_edit::copyable(&value))
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_initialize<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        context: JObject<'a>,
    ) {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let package = env
                .call_method(
                    &context,
                    jni_str!("getPackageName"),
                    jni_sig!(() -> java.lang.String),
                    &[],
                )?
                .l()?;
            let package = JString::cast_local(env, package)?.to_string();
            let channel = if package.ends_with(".debug") {
                zork_client_core::channel::Channel::Dev
            } else {
                zork_client_core::channel::Channel::Release
            };
            zork_client_core::channel::set_host_channel(channel)
                .expect("consistent application channel");
            rustls_platform_verifier::android::init_with_env(env, context)
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_clearData<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        root: JString<'a>,
        confirmed: jni::sys::jboolean,
        context: JObject<'a>,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let result = (|| -> anyhow::Result<()> {
                let directory = env
                    .call_method(
                        &context,
                        jni_str!("getNoBackupFilesDir"),
                        jni_sig!(() -> java.io.File),
                        &[],
                    )?
                    .l()?;
                let path = env
                    .call_method(
                        directory,
                        jni_str!("getAbsolutePath"),
                        jni_sig!(() -> java.lang.String),
                        &[],
                    )?
                    .l()?;
                let path = JString::cast_local(env, path)?.to_string();
                let root = std::path::PathBuf::from(root.to_string());
                anyhow::ensure!(
                    root == std::path::Path::new(&path).join("client"),
                    "只能清空当前应用的数据"
                );
                let host = super::host(&root)?;
                host.local.data_reset().clear(confirmed, || {
                    let service = JString::from_str(env, "activity")?;
                    let manager = env
                        .call_method(
                            &context,
                            jni_str!("getSystemService"),
                            jni_sig!((java.lang.String) -> java.lang.Object),
                            &[JValue::Object(service.as_ref())],
                        )?
                        .l()?;
                    // The OS terminates every process belonging to this package
                    // and clears preferences, files, databases and caches together.
                    let accepted = env
                        .call_method(
                            manager,
                            jni_str!("clearApplicationUserData"),
                            jni_sig!(() -> boolean),
                            &[],
                        )?
                        .z()?;
                    anyhow::ensure!(accepted, "系统未接受清空数据请求，请重试");
                    Ok(())
                })
            })();
            let reply = match result {
                Ok(()) => serde_json::json!({"ok":true}),
                Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}),
            };
            JString::from_str(env, reply.to_string())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_call<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        root: JString<'a>,
        request: JString<'a>,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let reply = super::response(
                std::path::Path::new(&root.to_string()),
                &request.to_string(),
            );
            JString::from_str(env, reply)
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_sharedFileBytes<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        root: JString<'a>,
        content: JString<'a>,
    ) -> jni::objects::JByteArray<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let bytes = super::host(std::path::Path::new(&root.to_string()))
                .ok()
                .and_then(|host| {
                    host.local
                        .shared_files()
                        .preview_bytes(&content.to_string())
                });
            env.byte_array_from_slice(bytes.as_deref().map(Vec::as_slice).unwrap_or_default())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_saveSharedFile<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        root: JString<'a>,
        ticket: JString<'a>,
        descriptor: jni::sys::jint,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let result = (|| -> anyhow::Result<()> {
                use std::os::fd::BorrowedFd;
                anyhow::ensure!(descriptor >= 0, "无效的保存位置");
                // Android retains its ParcelFileDescriptor; this adapter owns a
                // duplicate for the duration of the core-controlled write.
                let owned = unsafe { BorrowedFd::borrow_raw(descriptor) }.try_clone_to_owned()?;
                let mut file = std::fs::File::from(owned);
                super::host(std::path::Path::new(&root.to_string()))?
                    .local
                    .shared_files()
                    .write_copy(&ticket.to_string(), &mut file)
            })();
            let reply = match result {
                Ok(()) => serde_json::json!({"ok":true}),
                Err(e) => serde_json::json!({"ok":false,"error":e.to_string()}),
            };
            JString::from_str(env, reply.to_string())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_chatFileBytes<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        root: JString<'a>,
        content: JString<'a>,
    ) -> jni::objects::JByteArray<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let bytes = super::host(std::path::Path::new(&root.to_string()))
                .ok()
                .and_then(|host| host.local.chat_files().preview_bytes(&content.to_string()));
            env.byte_array_from_slice(bytes.as_deref().map(Vec::as_slice).unwrap_or_default())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_ing_zork_android_NativeBridge_saveChatFile<'a>(
        mut env: EnvUnowned<'a>,
        _this: JObject<'a>,
        root: JString<'a>,
        ticket: JString<'a>,
        descriptor: jni::sys::jint,
    ) -> JString<'a> {
        env.with_env(|env| -> Result<_, jni::errors::Error> {
            let result = (|| -> anyhow::Result<()> {
                use std::os::fd::BorrowedFd;
                anyhow::ensure!(descriptor >= 0, "无效的保存位置");
                // Android retains its ParcelFileDescriptor; this adapter owns a
                // duplicate for the duration of the core-controlled write.
                let owned = unsafe { BorrowedFd::borrow_raw(descriptor) }.try_clone_to_owned()?;
                let mut file = std::fs::File::from(owned);
                super::host(std::path::Path::new(&root.to_string()))?
                    .local
                    .chat_files()
                    .write_copy(&ticket.to_string(), &mut file)
            })();
            let reply = match result {
                Ok(()) => serde_json::json!({"ok":true}),
                Err(e) => serde_json::json!({"ok":false,"error":e.to_string()}),
            };
            JString::from_str(env, reply.to_string())
        })
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
    }
}
