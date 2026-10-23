use std::ffi::{CString, c_char, c_int};

use easytier::common::config::{ConfigFileControl, TomlConfigLoader};

use crate::{
    config_server::{
        in_config_server_callback, reject_managed_delete_or_retain, wait_for_config_server_delivery,
    },
    error::set_error_msg,
    state::{ffi_context, resolve_instance_id_by_name},
    types::KeyValuePair,
};

/// # Safety
/// Set the tun fd
pub(crate) unsafe fn set_tun_fd(inst_name: *const c_char, fd: c_int) -> c_int {
    let inst_name = unsafe {
        assert!(!inst_name.is_null());
        std::ffi::CStr::from_ptr(inst_name)
            .to_string_lossy()
            .into_owned()
    };
    let inst_id = match resolve_instance_id_by_name(&inst_name) {
        Ok(Some(instance_id)) => instance_id,
        Ok(None) => {
            set_error_msg("instance not found");
            return -1;
        }
        Err(error) => {
            set_error_msg(&error.to_string());
            return -1;
        }
    };

    match ffi_context().manager.attach_tun_fd(inst_id, fd) {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

/// # Safety
/// Parse the config
pub(crate) unsafe fn parse_config(cfg_str: *const std::ffi::c_char) -> std::ffi::c_int {
    let cfg_str = unsafe {
        assert!(!cfg_str.is_null());
        std::ffi::CStr::from_ptr(cfg_str)
            .to_string_lossy()
            .into_owned()
    };

    if let Err(e) = TomlConfigLoader::new_from_str(&cfg_str) {
        set_error_msg(&format!("failed to parse config: {:?}", e));
        return -1;
    }

    0
}

/// # Safety
/// Run the network instance
pub(crate) unsafe fn run_network_instance(cfg_str: *const std::ffi::c_char) -> std::ffi::c_int {
    if in_config_server_callback() {
        set_error_msg("cannot run network instance from config server callback");
        return -1;
    }

    let cfg_str = unsafe {
        assert!(!cfg_str.is_null());
        std::ffi::CStr::from_ptr(cfg_str)
            .to_string_lossy()
            .into_owned()
    };
    let cfg = match TomlConfigLoader::new_from_str(&cfg_str) {
        Ok(cfg) => cfg,
        Err(e) => {
            set_error_msg(&format!("failed to parse config: {}", e));
            return -1;
        }
    };

    wait_for_config_server_delivery();
    if let Err(e) = ffi_context().runtime.block_on(
        ffi_context()
            .process_management
            .run_owned_network_instance(cfg, ConfigFileControl::STATIC_CONFIG),
    ) {
        set_error_msg(&format!("failed to start instance: {}", e));
        return -1;
    }

    0
}

unsafe fn parse_instance_names(
    inst_names: *const *const c_char,
    length: usize,
) -> Option<Vec<String>> {
    if length == 0 {
        return Some(Vec::new());
    }
    if inst_names.is_null() {
        set_error_msg("inst_names is null");
        return None;
    }

    let names = unsafe { std::slice::from_raw_parts(inst_names, length) };
    let mut parsed = Vec::with_capacity(length);
    for (index, &name) in names.iter().enumerate() {
        if name.is_null() {
            set_error_msg(&format!("inst_names[{}] is null", index));
            return None;
        }
        parsed.push(
            unsafe { std::ffi::CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned(),
        );
    }
    Some(parsed)
}

/// # Safety
/// Retain the network instance
pub(crate) unsafe fn retain_network_instance(
    inst_names: *const *const std::ffi::c_char,
    length: usize,
) -> std::ffi::c_int {
    if in_config_server_callback() {
        set_error_msg("cannot retain network instances from config server callback");
        return -1;
    }

    wait_for_config_server_delivery();
    let retained_names = if length == 0 {
        Vec::new()
    } else {
        let Some(inst_names) = (unsafe { parse_instance_names(inst_names, length) }) else {
            return -1;
        };
        inst_names
    };

    // Gate under mutation_lock via gated API (closes TOCTOU vs external check).
    if let Err(error) = ffi_context().runtime.block_on(
        ffi_context()
            .process_management
            .retain_owned_network_instances_by_name_gated(
                retained_names,
                reject_managed_delete_or_retain,
            ),
    ) {
        set_error_msg(&format!("failed to retain instances: {error}"));
        return -1;
    }

    0
}

/// Shared body for stop / (gated) delete: `delete_owned_*` = mark disabled + detach.
unsafe fn stop_owned_by_name(
    inst_names: *const *const std::ffi::c_char,
    length: usize,
    from_callback_msg: &str,
    fail_msg: &str,
) -> std::ffi::c_int {
    if in_config_server_callback() {
        set_error_msg(from_callback_msg);
        return -1;
    }

    wait_for_config_server_delivery();
    if length == 0 {
        return 0;
    }

    let Some(inst_names) = (unsafe { parse_instance_names(inst_names, length) }) else {
        return -1;
    };

    if let Err(error) = ffi_context().runtime.block_on(
        ffi_context()
            .process_management
            .delete_owned_network_instances_by_name(inst_names),
    ) {
        set_error_msg(&format!("{fail_msg}: {error}"));
        return -1;
    }

    0
}

/// # Safety
/// Stop named network instances (disable + detach). Always allowed for managed
/// rows while config-server is active — this is the intentional local stop path
/// (§1「停用保留」). Prefer this over [`delete_network_instance`] for stop UX.
pub(crate) unsafe fn stop_network_instance(
    inst_names: *const *const std::ffi::c_char,
    length: usize,
) -> std::ffi::c_int {
    unsafe {
        stop_owned_by_name(
            inst_names,
            length,
            "cannot stop network instances from config server callback",
            "failed to stop instances",
        )
    }
}

/// # Safety
/// Delete named network instances.
///
/// Internally still uses stop semantics (`delete_owned_*`), but **rejects**
/// console-managed ids while config-server is active. Hosts that only need
/// disable must call [`stop_network_instance`] instead.
pub(crate) unsafe fn delete_network_instance(
    inst_names: *const *const std::ffi::c_char,
    length: usize,
) -> std::ffi::c_int {
    if in_config_server_callback() {
        set_error_msg("cannot delete network instances from config server callback");
        return -1;
    }

    wait_for_config_server_delivery();
    if length == 0 {
        return 0;
    }

    let Some(inst_names) = (unsafe { parse_instance_names(inst_names, length) }) else {
        return -1;
    };

    // Gate under mutation_lock via gated API (closes TOCTOU vs external check).
    if let Err(error) = ffi_context().runtime.block_on(
        ffi_context()
            .process_management
            .delete_owned_network_instances_by_name_gated(
                inst_names,
                reject_managed_delete_or_retain,
            ),
    ) {
        set_error_msg(&format!("failed to delete instances: {error}"));
        return -1;
    }

    0
}

/// # Safety
/// Collect the network infos
pub(crate) unsafe fn collect_network_infos(
    infos: *mut KeyValuePair,
    max_length: usize,
) -> std::ffi::c_int {
    if in_config_server_callback() {
        set_error_msg("cannot collect network infos from config server callback");
        return -1;
    }

    if max_length == 0 {
        return 0;
    }

    let infos = unsafe {
        assert!(!infos.is_null());
        std::slice::from_raw_parts_mut(infos, max_length)
    };

    let collected_infos = match ffi_context().manager.collect_network_infos_sync(None) {
        Ok(infos) => infos,
        Err(e) => {
            set_error_msg(&format!("failed to collect network infos: {}", e));
            return -1;
        }
    };

    let mut index = 0;
    for (instance_id, value) in collected_infos.iter() {
        if index >= max_length {
            break;
        }
        let Some(key) = ffi_context()
            .manager
            .instance(*instance_id)
            .map(|instance| instance.instance_name().to_owned())
        else {
            continue;
        };
        // convert value to json string
        let value = match serde_json::to_string(&value) {
            Ok(value) => value,
            Err(e) => {
                set_error_msg(&format!("failed to serialize instance info: {}", e));
                return -1;
            }
        };

        infos[index] = KeyValuePair {
            key: std::ffi::CString::new(key).unwrap().into_raw(),
            value: std::ffi::CString::new(value).unwrap().into_raw(),
        };
        index += 1;
    }

    index as std::ffi::c_int
}

/// # Safety
/// List the instance names and IDs known by the FFI instance manager.
pub(crate) unsafe fn list_instance(infos: *mut KeyValuePair, max_length: usize) -> std::ffi::c_int {
    if in_config_server_callback() {
        set_error_msg("cannot list instances from config server callback");
        return -1;
    }

    if max_length == 0 {
        return 0;
    }

    if infos.is_null() {
        set_error_msg("infos is null");
        return -1;
    }

    let infos = unsafe { std::slice::from_raw_parts_mut(infos, max_length) };
    let mut instances = ffi_context()
        .manager
        .instance_ids()
        .into_iter()
        .filter_map(|id| {
            ffi_context()
                .manager
                .instance(id)
                .map(|instance| (instance.instance_name().to_owned(), id))
        })
        .collect::<Vec<_>>();
    instances.sort_by(|(left_name, left_id), (right_name, right_id)| {
        left_name
            .cmp(right_name)
            .then_with(|| left_id.to_string().cmp(&right_id.to_string()))
    });

    let encoded_instances = match instances
        .into_iter()
        .take(max_length)
        .map(|(name, id)| {
            let key = CString::new(name)
                .map_err(|err| format!("failed to encode instance name: {}", err))?;
            let value = CString::new(id.to_string())
                .map_err(|err| format!("failed to encode instance id: {}", err))?;
            Ok((key, value))
        })
        .collect::<Result<Vec<_>, String>>()
    {
        Ok(value) => value,
        Err(err) => {
            set_error_msg(&err);
            return -1;
        }
    };

    let count = encoded_instances.len();
    for (index, (key, value)) in encoded_instances.into_iter().enumerate() {
        infos[index] = KeyValuePair {
            key: key.into_raw(),
            value: value.into_raw(),
        };
    }

    count as std::ffi::c_int
}
