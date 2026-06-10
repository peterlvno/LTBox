//! Root-wizard handler (family/mode/provider/version, confirm, exec). Extracted from `main.rs`.

use crate::*;
use iced::Task;
use ltbox_core::tr_args;

impl App {
    #[allow(unreachable_code)]
    pub(crate) fn update_root(&mut self, msg: RootMsg) -> Task<Message> {
        match msg {
            RootMsg::RootFamily(f) => {
                // Defense in depth: the family card UI grays out the
                // Magisk option on TB320FC, but a stale message from a
                // pre-poll click could still land here. Drop it so the
                // wizard never enters a configuration we know boot-loops.
                if self.is_tb320fc() && f == Family::Magisk {
                    return Task::none();
                }
                self.root.family = Some(f);
                self.root.provider = None;
                self.root.mode = None;
                self.root.file_path = None;
                self.root.kernel_version = None;
                Task::none()
            }
            RootMsg::RootProvider(p) => {
                self.root.provider = Some(p);
                self.root.file_path = None;
                // ReSukiSU has no Stable channel — if the user had Stable
                // picked before switching to ReSukiSU, force Nightly so the
                // hidden-Stable version step lands on the sole valid choice
                // instead of showing an orphan "no selection" state.
                if p == Provider::ReSukiSU && self.root.version == Some(VerChoice::Stable) {
                    self.root.version = Some(VerChoice::Nightly);
                    self.root.nightly_source = None;
                    self.root.run_id = None;
                    self.root.run_id_buffer.clear();
                }
                Task::none()
            }
            RootMsg::RootMode(m) => {
                // TODO(root): TB320FC has no init_boot for the current
                // KernelSU LKM path; replace it with a vendor_boot patch
                // once real-device verification is available. Block stale
                // messages while the visible card stays disabled.
                if self.is_tb320fc() && m == RootMode::Lkm {
                    return Task::none();
                }
                // TODO(root): LTBox currently only swaps the boot.img Image
                // for GKI, which corrupts boot on TB323FU. Keep it disabled
                // until vbmeta handling is added.
                if self.is_tb323fu() && m == RootMode::Gki {
                    return Task::none();
                }
                self.root.mode = Some(m);
                self.root.file_path = None;
                self.root.kernel_version = None;
                Task::none()
            }
            RootMsg::RootVersion(v) => {
                self.root.version = Some(v);
                self.root.nightly_source = None;
                self.root.run_id = None;
                self.root.run_id_buffer.clear();
                Task::none()
            }
            RootMsg::RootNightlySource(s) => {
                self.root.nightly_source = Some(s);
                match s {
                    NightlySource::AutoDetect => {
                        // Leaving ManualInput — drop the committed run ID.
                        self.root.run_id = None;
                        self.root.run_id_buffer.clear();
                    }
                    NightlySource::ManualInput => {
                        // Prefill from any previous commit so re-entry is painless.
                        self.root.run_id_buffer = self.root.run_id.clone().unwrap_or_default();
                        self.root.run_id_popup_open = true;
                    }
                }
                Task::none()
            }
            RootMsg::RootSelectFile => {
                self.picker_target = PickerTarget::RootFile;
                let spec = if self.root.is_gki() {
                    // GKI route accepts both an AnyKernel3 zip and a raw
                    // boot.img — the patcher branches on the extension
                    // (`gki::patch_boot` unpacks the .img with
                    // magiskboot in a scratch subdir to pull the kernel
                    // out, then reuses the existing repack path).
                    pickers::FilePickSpec::single("picker_target_kernel_image")
                        .with_filter("Kernel image", &["zip", "img"])
                } else {
                    pickers::FilePickSpec::single("picker_target_apatch_apk")
                        .with_filter("APK", &["apk"])
                };
                pickers::pick_file_for(spec, &self.recent_paths, Message::FileSelected)
            }
            RootMsg::RootSelectFolder => {
                // Historical field name; value is now a single EDL loader file.
                if let Some(path) = self.resolved_default_loader() {
                    // A fitting Settings default loader bypasses the picker; a
                    // model-mismatched (or missing) one falls through to it.
                    self.root.folder_path = Some(path);
                    return Task::none();
                }
                self.picker_target = PickerTarget::RootLoader;
                return pickers::pick_file_for(
                    loader_file_spec("picker_target_edl_loader"),
                    &self.recent_paths,
                    Message::FileSelected,
                );
                Task::none()
            }
            RootMsg::RootNext => {
                if self.root.step == 6 {
                    if self.root.needs_ksu_lkm_kernel_version() {
                        // ADB probe is blocking — push to the heavy pool so
                        // the UI doesn't freeze on a slow / unresponsive
                        // device. Continuation lands in
                        // `RootKernelVersionProbeDone`.
                        return task_heavy(
                            || {
                                ltbox_device::adb::AdbManager::new_if_connected().and_then(
                                    |mut adb| {
                                        adb.get_kernel_version().ok().flatten().and_then(|kv| {
                                        ltbox_patch::root_pipeline::normalize_ksu_kernel_version(
                                            &kv,
                                        )
                                    })
                                    },
                                )
                            },
                            |__v| Message::Root(RootMsg::RootKernelVersionProbeDone(__v)),
                            |_e| None,
                        );
                    }
                    self.root.next();
                    return self.update(Message::Root(RootMsg::RootExecStart));
                }
                // APatch KPM step: advance only after superkey confirmation.
                if self.root.step == 8 {
                    self.root.superkey_buffer.clear();
                    self.root.superkey_first_entry = None;
                    self.root.superkey_popup_open = true;
                    return Task::none();
                }
                self.root.next();
                // Skip loader step when a valid Settings default exists.
                if self.root.step == 5
                    && self.root.folder_path.is_none()
                    && let Some(path) = self.resolved_default_loader()
                {
                    self.root.folder_path = Some(path);
                    self.root.next();
                }
                Task::none()
            }
            RootMsg::RootBack => {
                self.root.back();
                Task::none()
            }
            RootMsg::RootSelectKpm => {
                // Multi-select; paths merge-dedup into the list so
                // the user can Browse multiple times.
                let spec = pickers::FilePickSpec::multi("picker_target_kpm_modules")
                    .with_filter("KPM modules", &["kpm"]);
                return pickers::pick_files_for(spec, &self.recent_paths, |__v| {
                    Message::Root(RootMsg::RootKpmSelected(__v))
                });
                Task::none()
            }
            RootMsg::RootKpmSelected(paths) => {
                if let Some(paths) = paths {
                    if let Some(first) = paths.first() {
                        self.remember_recent(pickers::PickerKind::File, first);
                    }
                    for p in paths {
                        if !self.root.kpm_paths.iter().any(|existing| existing == &p) {
                            self.root.kpm_paths.push(p);
                        }
                    }
                }
                Task::none()
            }
            RootMsg::RootKpmRemove(path) => {
                self.root.kpm_paths.retain(|p| p != &path);
                Task::none()
            }
            RootMsg::RootSuperkeyInput(text) => {
                self.root.superkey_buffer = text;
                Task::none()
            }
            RootMsg::RootSuperkeyConfirm => {
                let key = self.root.superkey_buffer.trim().to_string();
                match self.root.superkey_first_entry.take() {
                    None => {
                        // Stage 1 — first entry. Validate the format
                        // up-front so the user finds out about a too-short
                        // / non-alnum key on the first round, not after
                        // re-typing it. Upstream rule: 8–63 alphanumeric.
                        let valid = (8..=63).contains(&key.len())
                            && key.chars().all(|c| c.is_ascii_alphanumeric());
                        if !valid {
                            self.error_msg = Some(self.t("apatch_superkey_invalid").to_string());
                            return Task::none();
                        }
                        // Stash the validated first entry, blank the
                        // field, and stay open for the verification
                        // round. View flips to the "re-enter" prompt
                        // because `superkey_first_entry.is_some()`.
                        self.root.superkey_first_entry = Some(key);
                        self.root.superkey_buffer.clear();
                        self.error_msg = None;
                    }
                    Some(first) => {
                        // Stage 2 — verification entry. Mismatch resets
                        // the whole flow so the user types both rounds
                        // again from scratch (no "edit second field"
                        // shortcut, since the typo could be in either).
                        if key != first {
                            self.error_msg = Some(self.t("apatch_superkey_mismatch").to_string());
                            self.root.superkey_buffer.clear();
                            // `superkey_first_entry` already cleared by
                            // the `.take()` above — stage flips back to
                            // first-entry automatically.
                            return Task::none();
                        }
                        self.root.superkey = Some(key);
                        self.root.superkey_buffer.clear();
                        self.root.superkey_popup_open = false;
                        self.error_msg = None;
                        self.root.next();
                    }
                }
                Task::none()
            }
            RootMsg::RootSuperkeyCancel => {
                self.root.superkey_buffer.clear();
                self.root.superkey_first_entry = None;
                self.root.superkey_popup_open = false;
                self.error_msg = None;
                Task::none()
            }
            RootMsg::RootRunIdInput(text) => {
                // GH Actions run IDs are 10 digits; cap at 12 for headroom.
                let filtered: String = text
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .take(12)
                    .collect();
                self.root.run_id_buffer = filtered;
                Task::none()
            }
            RootMsg::RootRunIdConfirm => {
                let id = self.root.run_id_buffer.trim().to_string();
                if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
                    self.error_msg = Some(self.t("nightly_manual_invalid").to_string());
                    return Task::none();
                }
                self.root.run_id = Some(id);
                self.root.run_id_popup_open = false;
                self.error_msg = None;
                Task::none()
            }
            RootMsg::RootRunIdCancel => {
                self.root.run_id_buffer.clear();
                self.root.run_id_popup_open = false;
                // Roll back NightlySource so the step gate forces a re-pick.
                if self.root.run_id.is_none() {
                    self.root.nightly_source = None;
                }
                Task::none()
            }
            RootMsg::RootKernelVersionInput(text) => {
                let filtered: String = text
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.')
                    .take(16)
                    .collect();
                self.root.kernel_version_buffer = filtered;
                Task::none()
            }
            RootMsg::RootKernelVersionConfirm => {
                let input = self.root.kernel_version_buffer.trim();
                let Some(kv) = ltbox_patch::root_pipeline::normalize_ksu_kernel_version(input)
                else {
                    self.error_msg = Some(self.t("root_kernel_version_invalid").to_string());
                    return Task::none();
                };
                self.root.kernel_version = Some(kv);
                self.root.kernel_version_buffer.clear();
                self.root.kernel_version_popup_open = false;
                self.error_msg = None;
                if self.root.step == 6 {
                    self.root.next();
                    return self.update(Message::Root(RootMsg::RootExecStart));
                }
                Task::none()
            }
            RootMsg::RootKernelVersionCancel => {
                self.root.kernel_version_buffer.clear();
                self.root.kernel_version_popup_open = false;
                Task::none()
            }
            RootMsg::RootKernelVersionProbeDone(detected) => {
                // Wizard may have moved off step 6 by the time the probe
                // returns (user clicked Back); only act if still at the
                // same gating point.
                if self.root.step != 6 || !self.root.needs_ksu_lkm_kernel_version() {
                    return Task::none();
                }
                if let Some(kv) = detected {
                    self.root.kernel_version = Some(kv);
                    self.root.next();
                    return self.update(Message::Root(RootMsg::RootExecStart));
                }
                self.root.kernel_version_buffer =
                    self.root.kernel_version.clone().unwrap_or_default();
                self.root.kernel_version_popup_open = true;
                Task::none()
            }
            RootMsg::RootExecStart => {
                // TODO(root): LTBox currently only swaps the boot.img Image
                // for GKI, which corrupts boot on TB323FU. The mode card is
                // disabled, but stale selections can survive from before the
                // model was identified; refuse them until vbmeta handling is
                // added.
                if self.is_tb323fu() && self.root.is_gki() {
                    self.root.mode = None;
                    self.root.step = 1; // Mode step
                    self.error_msg = Some(tr_args!("model_unsupported", model = "TB323FU"));
                    return Task::none();
                }
                if self
                    .validate_loader_path(&self.root.folder_path.clone())
                    .is_err()
                {
                    return Task::none();
                }
                self.begin_op(View::Root);
                self.op_steps = self.derive_root_op_steps();
                self.error_msg = None;
                let family = self.root.family;
                let mode = self.root.mode;
                let provider = self.root.provider;
                let version = self.root.version;
                let file_path = self.root.file_path.clone();
                let gui_kernel_version = self.root.kernel_version.clone();
                let conn = self.connection;
                // Folder must contain `xbl_s_devprg_ns.melf`; optional
                // `keys/testkey_rsa{2048,4096}.pem` as KEY_MAP fallback.
                let fw_folder = self.root.folder_path.clone();
                // APatch-only; empty / default elsewhere.
                let kpm_paths: Vec<std::path::PathBuf> = self
                    .root
                    .kpm_paths
                    .iter()
                    .map(std::path::PathBuf::from)
                    .collect();
                let superkey = self.root.superkey.clone().unwrap_or_default();
                let nightly_run_id: Option<u64> =
                    if self.root.nightly_source == Some(NightlySource::ManualInput) {
                        self.root.run_id.as_deref().and_then(|s| s.parse().ok())
                    } else {
                        None
                    };

                let fam_label = family
                    .map(|f| self.t(f.label_key()).to_string())
                    .unwrap_or_else(|| "?".to_string());
                self.log_push(format!(
                    "[Root] {}",
                    tr_args!("log_op_starting", what = fam_label)
                ));
                // Resolve Magisk preinit device via /proc/self/mountinfo
                // before ADB vanishes past EDL. Gates /data on the device's
                // encryption state — metadata-encrypted devices land preinit
                // on userdata otherwise and boot-loop after first wipe.
                let preinit_device: String = if matches!(family, Some(Family::Magisk))
                    && matches!(
                        self.connection,
                        ConnectionStatus::Adb | ConnectionStatus::AdbRecovery
                    ) {
                    let (mountinfo, encrypt_type) = if let Some(mut adb) =
                        ltbox_device::adb::AdbManager::new_if_connected()
                    {
                        let mi = adb.shell("cat /proc/self/mountinfo").unwrap_or_default();
                        let cs = adb.shell("getprop ro.crypto.state").unwrap_or_default();
                        let ct = adb.shell("getprop ro.crypto.type").unwrap_or_default();
                        let cme = adb
                            .shell("getprop ro.crypto.metadata.enabled")
                            .unwrap_or_default();
                        (
                            mi,
                            ltbox_patch::magisk::derive_encrypt_type(&cs, &ct, &cme).to_string(),
                        )
                    } else {
                        (String::new(), String::from("file"))
                    };
                    if mountinfo.is_empty() {
                        self.log_push(format!(
                            "[Magisk] {}",
                            ltbox_core::i18n::tr("log_magisk_preinit_adb_unavailable")
                        ));
                        String::new()
                    } else {
                        self.log_push(format!(
                            "[Magisk] {}",
                            tr_args!("log_magisk_crypto_state", encrypt_type = encrypt_type)
                        ));
                        match ltbox_patch::magisk::resolve_preinit_device(&mountinfo, &encrypt_type)
                        {
                            Some(name) => {
                                self.log_push(format!(
                                    "[Magisk] {}",
                                    tr_args!("log_magisk_preinit_resolved", name = name)
                                ));
                                name
                            }
                            None => {
                                self.log_push(format!(
                                    "[Magisk] {}",
                                    ltbox_core::i18n::tr("log_magisk_preinit_none")
                                ));
                                String::new()
                            }
                        }
                    }
                } else {
                    String::new()
                };
                let ll = self.live_labels();

                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            ltbox_core::runtime::run_heavy(move || {
                                root_worker(
                                    family,
                                    mode,
                                    provider,
                                    version,
                                    file_path,
                                    gui_kernel_version,
                                    conn,
                                    fw_folder,
                                    kpm_paths,
                                    superkey,
                                    nightly_run_id,
                                    preinit_device,
                                    ll,
                                )
                            })
                            .and_then(|r| r)
                        })
                        .await
                        .unwrap_or(Err("Task failed".to_string()))
                    },
                    |result| match result {
                        Ok(lines) => Message::Root(RootMsg::RootExecDone(lines)),
                        Err(e) => Message::OperationError(e),
                    },
                );
                Task::none()
            }
            RootMsg::RootExecDone(lines) => {
                self.flush_exec_done_log(lines);
                self.end_op();
                Task::none()
            }
        }
    }
}
