<script setup lang="ts">
import { Button, Message, Select, Tag, useConfirm, useToast, type VirtualScrollerLazyEvent } from 'primevue';
import { TOAST_LIFE } from '../modules/toast'
import { computed, onMounted, onUnmounted, ref, toRaw, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import * as Api from '../modules/api';
import {
    captureNormalizedDirtySnapshot,
    configDirtySnapshot,
    isConfigSnapshotDirty,
} from '../modules/config-dirty';
import * as Utils from '../modules/utils';
import * as NetworkTypes from '../types/network';
import LoggingSettingsDialog from './LoggingSettingsDialog.vue';
import type { LoggingSettingsApi } from '../modules/logging';

const { t } = useI18n()

const props = defineProps<{
    api: Api.RemoteClient;
    newConfigGenerator?: () => NetworkTypes.NetworkConfig;
    pauseAutoRefresh?: boolean;
    /** Web Drawer / GUI ???????????????????? */
    drawerClose?: () => void;
    /** ????????? t('close') */
    leaveButtonLabel?: string;
    /** ???????? */
    leaveButtonTooltip?: string;
    /** ????????? pi-times */
    leaveButtonIcon?: string;
    /** Web ????????????????????? */
    fullPage?: boolean;
    /**
     * Web ???????status=??/???config=??/???
     * ????? GUI ????+????????
     */
    mode?: 'status' | 'config';
    /** ??????????????????????? */
    pageTitle?: string;
    /** Heartbeat fields for B6 DNS coverage badge (device management page). */
    deviceOsType?: string;
    deviceEasytierVersion?: string;
    deviceMagicDnsOsWired?: boolean | null;
}>();

const isStatusMode = computed(() => props.mode === 'status')
const isConfigMode = computed(() => props.mode === 'config')
/** GUI ???? mode ?????? */
const isCombinedMode = computed(() => !props.mode)
const leaveLabel = computed(() => props.leaveButtonLabel || t('close'))
const leaveTooltip = computed(() => props.leaveButtonTooltip || leaveLabel.value)
const leaveIcon = computed(() => props.leaveButtonIcon || 'pi pi-times')
/** ????????????/????????????????????? */
const showLeaveInFooter = computed(() => !!props.drawerClose)

const instanceId = defineModel('instanceId', {
    type: String as () => string | undefined,
    required: false,
})

const emits = defineEmits<{
    update: []
    switchMode: [mode: 'status' | 'config']
}>();

const toast = useToast();
const confirm = useConfirm();

function errorDetail(error: unknown): string {
    return Utils.formatApiErrorDetail(error, t);
}

/** False after unmount — blocks late probes / confirms on a destroyed instance. */
let managementAlive = true;

function forceReloadLatestConfig() {
    if (!managementAlive) {
        return;
    }
    void loadCurrentNetworkConfig({ force: true }).catch((reloadError) => {
        if (!managementAlive) {
            return;
        }
        console.error(reloadError);
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: t('web.device_management.load_config_failed') + ': ' + errorDetail(reloadError),
            life: TOAST_LIFE.error,
        });
    });
}

/** One-click reload after CAS conflict; returns true when the dialog was shown. */
function offerReloadOnRevisionConflict(error: unknown): boolean {
    if (!managementAlive) {
        return false;
    }
    const payload = Utils.extractApiErrorPayload(error);
    if (Utils.classifyApiError(payload) !== 'revision_conflict') {
        return false;
    }
    const revision = payload.current_config_revision?.trim();
    const message = revision
        ? t('web.device_management.confirm_reload_on_revision_conflict_with_rev', { revision })
        : t('web.device_management.confirm_reload_on_revision_conflict');
    confirm.require({
        message,
        header: t('web.common.error'),
        icon: 'pi pi-refresh',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.device_management.reload_latest_config'),
            severity: 'warning',
        },
        accept: () => forceReloadLatestConfig(),
    });
    return true;
}

/** Avoid spamming stale-server prompts while the same dirty draft is open. */
let serverStaleNotifiedForBaseline: string | null = null;

function offerReloadOnServerConfigUpdated() {
    if (!managementAlive) {
        return;
    }
    confirm.require({
        message: t('web.device_management.confirm_reload_server_updated_while_dirty'),
        header: t('web.common.warning'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.device_management.keep_editing'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.device_management.reload_latest_config'),
            severity: 'warning',
        },
        accept: () => forceReloadLatestConfig(),
    });
}

async function refreshCleanConfigRevision(expectedGeneration: number) {
    if (typeof props.api.get_managed_config_revision !== 'function') {
        return;
    }
    try {
        const revision = await props.api.get_managed_config_revision!();
        if (!managementAlive || expectedGeneration !== configLoadGeneration) {
            return;
        }
        cleanConfigRevision.value =
            typeof revision === 'string' && revision.trim() ? revision.trim() : null;
    } catch (e) {
        console.debug('managed-config revision probe failed', e);
    }
}

async function checkServerConfigStaleWhileDirty() {
    if (!managementAlive || !isConfigDirty.value || !showConfigPanel.value) {
        serverStaleNotifiedForBaseline = null;
        return;
    }
    const selected = selectedInstanceId.value?.uuid;
    const baselineSnap = cleanConfigSnapshot.value;
    const probeGeneration = configLoadGeneration;
    if (!selected || !baselineSnap || baselineSnap.startsWith('\0')) {
        return;
    }
    const notifyKey = `${selected}:${cleanConfigRevision.value ?? baselineSnap}`;
    if (serverStaleNotifiedForBaseline === notifyKey) {
        return;
    }

    try {
        // Prefer lightweight SQLite revision (no device RPC). Fall back to full config compare.
        if (typeof props.api.get_managed_config_revision === 'function') {
            const remoteRevision = await props.api.get_managed_config_revision!();
            if (
                !managementAlive
                || probeGeneration !== configLoadGeneration
                || selectedInstanceId.value?.uuid !== selected
                || !isConfigDirty.value
            ) {
                return;
            }
            const remote =
                typeof remoteRevision === 'string' && remoteRevision.trim()
                    ? remoteRevision.trim()
                    : null;
            const baselineRev = cleanConfigRevision.value;
            // Only compare when both sides have a revision; otherwise skip probe.
            if (baselineRev && remote && remote !== baselineRev) {
                serverStaleNotifiedForBaseline = notifyKey;
                offerReloadOnServerConfigUpdated();
            }
            return;
        }

        const remote = await props.api.get_network_config(selected);
        if (
            !managementAlive
            || probeGeneration !== configLoadGeneration
            || selectedInstanceId.value?.uuid !== selected
            || !isConfigDirty.value
        ) {
            return;
        }
        const remoteSnapshot = configDirtySnapshot(
            captureNormalizedDirtySnapshot(remote).config,
        );
        if (remoteSnapshot === null || remoteSnapshot === baselineSnap) {
            return;
        }
        serverStaleNotifiedForBaseline = notifyKey;
        offerReloadOnServerConfigUpdated();
    } catch (e) {
        console.debug('stale-config probe failed', e);
    }
}

const configFile = ref();

const curNetworkInfo = ref<NetworkTypes.NetworkInstance | null>(null);

const showConfigEditDialog = ref(false);
const isEditingNetwork = ref(false); // Flag to indicate if we're in network editing mode
const currentNetworkConfig = ref<NetworkTypes.NetworkConfig | undefined>(undefined);
/** Canonical dirty baseline; retaken after load / save / run refresh. */
const cleanConfigSnapshot = ref<string | null>(null);
/** Machine managed-config revision captured with the clean snapshot (web console). */
const cleanConfigRevision = ref<string | null>(null);
const isConfigDirty = ref(false);

const applyLoadedNetworkConfig = (config: NetworkTypes.NetworkConfig) => {
    const { config: normalized, snapshot } = captureNormalizedDirtySnapshot(config);
    currentNetworkConfig.value = normalized;
    cleanConfigSnapshot.value = snapshot;
    // Reset before async probe so a late previous-machine revision cannot linger.
    cleanConfigRevision.value = null;
    isConfigDirty.value = false;
    serverStaleNotifiedForBaseline = null;
    void refreshCleanConfigRevision(configLoadGeneration);
};

const clearNetworkConfigDraft = () => {
    currentNetworkConfig.value = undefined;
    cleanConfigSnapshot.value = null;
    cleanConfigRevision.value = null;
    isConfigDirty.value = false;
    serverStaleNotifiedForBaseline = null;
};

watch(
    currentNetworkConfig,
    (cfg) => {
        isConfigDirty.value = isConfigSnapshotDirty(cfg, cleanConfigSnapshot.value);
    },
    { deep: true },
);

const listInstanceIdResponse = ref<Api.ListNetworkInstanceIdResponse | undefined>(undefined);

const runningInstanceIds = computed(() => new Set(
    (listInstanceIdResponse.value?.running_inst_ids ?? []).map(Utils.UuidToStr),
));
const disabledInstanceIds = computed(() => new Set(
    (listInstanceIdResponse.value?.disabled_inst_ids ?? []).map(Utils.UuidToStr),
));

const isRunning = (instanceId: string) => runningInstanceIds.value.has(instanceId);

const networkMetaCache = ref<Record<string, Api.NetworkMeta>>({});
const networkMetaLoadFailed = ref<Record<string, boolean>>({});
const loadNetworkMetas = async (instanceIds: string[]) => {
    const missingIds = instanceIds.filter(id => !networkMetaCache.value[id]);

    if (missingIds.length === 0) return;

    try {
        const response = await props.api.get_network_metas(missingIds);
        const metas = response.metas ?? {};
        Object.assign(networkMetaCache.value, metas);
        updateInstanceList();
        missingIds.forEach((id) => {
            if (metas[id]) {
                delete networkMetaLoadFailed.value[id];
            }
        });
    } catch (e) {
        missingIds.forEach((id) => {
            networkMetaLoadFailed.value[id] = true;
        });
        console.error("Failed to load network metas", e);
    }
};
const onLazyLoadNetworkMetas = async (event: VirtualScrollerLazyEvent) => {
    const instanceIds = instanceList.value
        .slice(event.first, event.last + 1)
        .map(item => item.uuid);
    await loadNetworkMetas(instanceIds);
};
const currentNetworkMeta = computed(() => {
    if (!instanceId.value) {
        return undefined;
    }
    return networkMetaCache.value[instanceId.value];
});
const currentNetworkControl = {
    remoteSave: computed(() => {
        const meta = currentNetworkMeta.value;
        return !!meta && Api.ConfigFilePermission.isRemoveSaveable(meta.config_permission);
    }),
    editable: computed(() => {
        const meta = currentNetworkMeta.value;
        return !!meta && Api.ConfigFilePermission.isEditable(meta.config_permission);
    }),
    deletable: computed(() => {
        const meta = currentNetworkMeta.value;
        return !!meta && Api.ConfigFilePermission.isDeletable(meta.config_permission);
    })
}

/** Save allowed when meta loaded and config is editable (not READ_ONLY). */
const canSaveConfig = computed(() => {
    if (!currentNetworkConfig.value) {
        return false;
    }
    if (!currentNetworkMeta.value || networkMetaLoadFailed.value[currentNetworkConfig.value.instance_id]) {
        return false;
    }
    return currentNetworkControl.editable.value;
});

const saveConfigDisabledReason = computed(() => {
    if (canSaveConfig.value) {
        return '';
    }
    if (!currentNetworkConfig.value) {
        return t('web.device_management.save_config_disabled_no_config');
    }
    if (!currentNetworkMeta.value || networkMetaLoadFailed.value[currentNetworkConfig.value.instance_id]) {
        return t('web.device_management.save_config_disabled_meta');
    }
    return t('web.device_management.save_config_disabled_readonly');
});

const saveConfigTooltip = computed(() =>
    canSaveConfig.value
        ? t('web.device_management.save_config_tip')
        : saveConfigDisabledReason.value,
);

const savingConfig = ref(false);

/** Status CollectNetworkInfo cadence; ≥2s leaves BidirectRpc room for config / heartbeat / history. */
const STATUS_POLL_MS = 2000
/** Config editor only needs occasional running/stopped tags; ~6s with STATUS_POLL_MS. */
const CONFIG_MODE_LIST_POLL_EVERY = 3
let pollTick = 0

const instanceList = ref<Array<{ uuid: string; meta?: Api.NetworkMeta }>>([]);
let instanceListSignature = '';
const updateInstanceList = () => {
    let insts = new Set<string>();
    let t = listInstanceIdResponse.value;
    if (t) {
        (t.running_inst_ids ?? []).forEach((u) => insts.add(Utils.UuidToStr(u)));
        (t.disabled_inst_ids ?? []).forEach((u) => insts.add(Utils.UuidToStr(u)));
    }

    const newList = Array.from(insts).map((instance: string) => {
        return {
            uuid: instance,
            meta: networkMetaCache.value[instance]
        };
    });

    const signature = newList
        .map(({ uuid, meta }) => `${uuid}\u0000${meta?.network_name ?? ''}\u0000${meta?.config_permission ?? ''}`)
        .join('\u0001');
    if (signature !== instanceListSignature) {
        instanceListSignature = signature;
        instanceList.value = newList;
    }
}
watch(listInstanceIdResponse, updateInstanceList, { deep: false });
watch(instanceList, async (newVal) => {
    if (newVal) {
        const instanceIds = new Set(newVal.map(item => item.uuid));
        Object.keys(networkMetaCache.value).forEach(id => {
            if (!instanceIds.has(id)) {
                delete networkMetaCache.value[id];
            }
        });
    }
    // ?? Drawer ?? instanceId ??????????????????
    const currentExists = !!instanceId.value && newVal?.some(item => item.uuid === instanceId.value);
    if (newVal?.length && (!instanceId.value || !currentExists)) {
        const running = newVal.find(item => isRunning(item.uuid));
        instanceId.value = (running ?? newVal[0]).uuid;
    } else if (!newVal?.length && instanceId.value) {
        instanceId.value = undefined;
    }
});

const selectedInstanceId = computed({
    get() {
        return instanceList.value.find((instance) => instance.uuid === instanceId.value);
    },
    set(value: { uuid: string } | null | undefined) {
        instanceId.value = value ? value.uuid : undefined;
    }
});

/** Programmatic selection (run/new/etc.) — skips dirty confirm. */
const setSelectedInstanceId = (uuid: string | undefined) => {
    instanceId.value = uuid;
};

/** Confirm before discarding a dirty draft; runs `onAccept` if clean or confirmed. */
const confirmDiscardIfDirty = (onAccept: () => void) => {
    if (!isConfigDirty.value || !showConfigPanel.value) {
        onAccept();
        return;
    }
    confirm.require({
        message: t('web.device_management.confirm_discard_on_switch'),
        header: t('web.device_management.discard_changes'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.device_management.discard_changes'),
            severity: 'danger',
        },
        accept: onAccept,
    });
};

/**
 * Top Select uses one-way binding + this handler so dirty drafts can be
 * confirmed before assignment (watch cannot block async).
 */
const onSelectInstanceId = (value: { uuid: string } | null | undefined) => {
    const nextUuid = value?.uuid;
    if (nextUuid === instanceId.value) {
        return;
    }
    confirmDiscardIfDirty(() => {
        instanceId.value = nextUuid;
    });
};

const selectedNetworkRunning = computed(() => {
    const id = selectedInstanceId.value?.uuid;
    return !!id && isRunning(id);
});
const selectedNetworkStopped = computed(() => {
    const id = selectedInstanceId.value?.uuid;
    return !!id && !isRunning(id);
});

watch(selectedInstanceId, async (newVal, oldVal) => {
    try {
        const loads: Array<Promise<unknown>> = []
        if (newVal?.uuid !== oldVal?.uuid && (networkIsDisabled.value || isEditingNetwork.value || isConfigMode.value)) {
            loads.push(loadCurrentNetworkConfig())
        } else {
            loads.push(loadCurrentNetworkInfo())
        }

        if (newVal?.uuid && !networkMetaCache.value[newVal.uuid]) {
            loads.push(loadNetworkMetas([newVal.uuid]))
        }
        await Promise.all(loads)
    } catch (e) {
        console.error('Failed to load selected network', e);
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: errorDetail(e),
            life: TOAST_LIFE.error,
        });
    }
});

const needShowNetworkStatus = computed(() => {
    if (isConfigMode.value) {
        return false;
    }
    if (!selectedInstanceId.value) {
        // nothing selected
        return false;
    }
    if (networkIsDisabled.value) {
        // network is disabled
        return false;
    }
    if (isEditingNetwork.value) {
        // editing network
        return false;
    }
    return true;
})

/**
 * 配置页：Web `mode=config`，或 combined / 显式进入编辑。
 * 停止态不再自动落在配置（见 config-vs-run-pages：运行空态 + 节点配置入口）。
 */
const showConfigPanel = computed(() => {
    if (isStatusMode.value) {
        return false;
    }
    if (isEditingNetwork.value) {
        return true;
    }
    return isConfigMode.value && !!selectedInstanceId.value;
})

const networkIsDisabled = computed(() => {
    if (!selectedInstanceId.value) {
        return false;
    }
    return disabledInstanceIds.value.has(selectedInstanceId.value.uuid);
});

/**
 * 运行页 · 已停止：只读配置预览 + 启动（Web status / GUI combined，未在编辑时）。
 */
const showStatusDisabledPanel = computed(() => {
    if (!selectedInstanceId.value || !networkIsDisabled.value || isEditingNetwork.value) {
        return false;
    }
    if (showConfigPanel.value) {
        return false;
    }
    return isStatusMode.value || isCombinedMode.value;
})

/** 停止态只读副本，避免 Config 内部归一化写回脏了真正的草稿 */
const statusDisabledConfigPreview = ref<NetworkTypes.NetworkConfig | null>(null)
watch(
    () => (showStatusDisabledPanel.value ? currentNetworkConfig.value : null),
    (cfg) => {
        statusDisabledConfigPreview.value = cfg
            ? structuredClone(toRaw(cfg)) as NetworkTypes.NetworkConfig
            : null
    },
    { immediate: true },
)

watch(networkIsDisabled, async (newVal, oldVal) => {
    if (newVal !== oldVal && newVal === true) {
        try {
            await loadCurrentNetworkConfig();
        } catch (e) {
            console.error('Failed to load disabled network config', e);
            toast.add({
                severity: 'error',
                summary: t('web.common.error'),
                detail: errorDetail(e),
                life: TOAST_LIFE.error,
            });
        }
    }
});

let currentConfigLoad: { instanceId: string; promise: Promise<void> } | undefined;
/** Bumped on every load (incl. force) so in-flight older responses cannot apply. */
let configLoadGeneration = 0;

const loadCurrentNetworkConfig = async (opts?: { force?: boolean }) => {
    const selected = selectedInstanceId.value?.uuid;
    if (!selected) {
        clearNetworkConfigDraft();
        return;
    }

    if (!opts?.force && currentConfigLoad?.instanceId === selected) {
        return currentConfigLoad.promise;
    }

    const loadGeneration = ++configLoadGeneration;

    // Keep previous draft visible until the new config arrives (avoid blank flash).
    const promise = (async () => {
        const ret = await props.api.get_network_config(selected);
        if (loadGeneration !== configLoadGeneration) {
            return;
        }
        if (selectedInstanceId.value?.uuid !== selected) {
            return;
        }
        // P1-1: do not silently overwrite an unsaved draft (unless force reload).
        if (
            !opts?.force
            && isConfigDirty.value
            && currentNetworkConfig.value?.instance_id === selected
        ) {
            const remoteSnapshot = configDirtySnapshot(
                captureNormalizedDirtySnapshot(ret).config,
            );
            const baseline = cleanConfigSnapshot.value;
            const notifyKey = `${selected}:${cleanConfigRevision.value ?? baseline}`;
            if (
                remoteSnapshot !== null
                && baseline
                && remoteSnapshot !== baseline
                && serverStaleNotifiedForBaseline !== notifyKey
            ) {
                serverStaleNotifiedForBaseline = notifyKey;
                offerReloadOnServerConfigUpdated();
            }
            return;
        }
        applyLoadedNetworkConfig(ret);
    })();
    currentConfigLoad = { instanceId: selected, promise };
    try {
        await promise;
    } finally {
        if (currentConfigLoad?.promise === promise) {
            currentConfigLoad = undefined;
        }
    }
}

const stopNetwork = async () => {
    if (!selectedInstanceId.value) {
        return;
    }

    try {
        await props.api.update_network_instance_state(selectedInstanceId.value.uuid, true);
        isEditingNetwork.value = false;
        await loadNetworkInstanceIds();
        await loadCurrentNetworkInfo();
        emits('update');
    } catch (e: any) {
        console.error(e);
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: t('web.device_management.stop_failed') + ': ' + errorDetail(e),
            life: TOAST_LIFE.error,
        });
    }
}

/** Start a disabled network: may push console (SQLite) config onto the device. */
const startNetwork = () => {
    if (!selectedInstanceId.value) {
        return;
    }
    confirm.require({
        message: t('web.device_management.confirm_start_overwrite'),
        header: t('web.device_management.start_network'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.device_management.start_network'),
            severity: 'success',
        },
        accept: () => { void doStartNetwork() },
    });
}

const doStartNetwork = async () => {
    if (!selectedInstanceId.value) {
        return;
    }

    try {
        await props.api.update_network_instance_state(selectedInstanceId.value.uuid, false);
        await loadNetworkInstanceIds();
        await loadCurrentNetworkInfo();
        emits('update');
    } catch (e: any) {
        console.error(e);
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: t('web.device_management.start_failed') + ': ' + errorDetail(e),
            life: TOAST_LIFE.error,
        });
    }
}

const requestSwitchMode = (mode: 'status' | 'config') => {
    // Leaving config with a dirty draft would be wiped on next ensureConfigModeEditing reload.
    if (mode === 'status' && isConfigDirty.value && showConfigPanel.value) {
        confirmDiscardIfDirty(() => {
            clearNetworkConfigDraft();
            emits('switchMode', mode);
        });
        return;
    }
    emits('switchMode', mode);
}

const requestLeave = () => {
    if (!props.drawerClose) {
        return;
    }
    confirmDiscardIfDirty(() => {
        props.drawerClose?.();
    });
}

/**
 * Web `mode=config`：无论运行中还是已停止都进入可编辑配置。
 * （停止态运行页只读预览走 showStatusDisabledPanel，不走这里。）
 */
const ensureConfigModeEditing = async () => {
    if (!isConfigMode.value || !selectedInstanceId.value) {
        return;
    }
    const selected = selectedInstanceId.value.uuid
    // Already have this instance's draft (e.g. selectedInstanceId watch just loaded it).
    if (currentNetworkConfig.value?.instance_id === selected) {
        isEditingNetwork.value = true
        return
    }
    try {
        await loadCurrentNetworkConfig();
        if (currentNetworkConfig.value?.instance_id === selected) {
            isEditingNetwork.value = true;
        }
    } catch (e: any) {
        console.error(e);
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: t('web.device_management.load_config_failed') + ': ' + errorDetail(e),
            life: TOAST_LIFE.error,
        });
    }
}

watch(
    () => [props.mode, selectedInstanceId.value?.uuid, networkIsDisabled.value] as const,
    async () => {
        if (isStatusMode.value) {
            isEditingNetwork.value = false;
            return;
        }
        if (isConfigMode.value) {
            pollTick = 0
            await ensureConfigModeEditing();
        }
    },
)

const confirmDeleteNetwork = () => {
    confirm.require({
        message: t('web.device_management.confirm_delete_network'),
        header: t('web.device_management.confirm_delete_network_header'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true
        },
        acceptProps: {
            label: t('web.device_management.delete_network'),
            severity: 'danger'
        },
        accept: async () => {
            try {
                await props.api.delete_network(instanceId.value!);
                toast.add({
                    severity: 'success',
                    summary: t('web.device_management.delete_network'),
                    detail: t('web.common.success'),
                    life: TOAST_LIFE.success,
                });
            } catch (e) {
                console.error(e);
                toast.add({
                    severity: 'error',
                    summary: t('web.device_management.delete_network'),
                    detail: errorDetail(e),
                    life: TOAST_LIFE.error,
                });
            }
            emits('update');
        },
        reject: () => {
            return;
        }
    });
};

const saveAndRunNewNetwork = async (config?: NetworkTypes.NetworkConfig) => {
    const cfg = config ?? currentNetworkConfig.value;
    if (!cfg) {
        return;
    }

    const targetInstanceId = instanceId.value ?? cfg.instance_id;
    if (targetInstanceId && cfg.instance_id !== targetInstanceId) {
        cfg.instance_id = targetInstanceId;
    }

    try {
        if (networkIsDisabled.value) {
            await props.api.save_config(cfg);
            await props.api.update_network_instance_state(cfg.instance_id, false);
        } else {
            await props.api.run_network(cfg, currentNetworkControl.remoteSave.value);
        }

        delete networkMetaCache.value[cfg.instance_id];
        await loadNetworkMetas([cfg.instance_id]);

        setSelectedInstanceId(cfg.instance_id);
        await loadNetworkInstanceIds();
        await loadCurrentNetworkInfo();

        // Windows may write back auto-assigned wintun `dev_name` during post_run.
        // Refresh so a subsequent Save does not push a stale empty value.
        // Guard instance_id: a concurrent selectedInstanceId watch load may race.
        try {
            const refreshed = await props.api.get_network_config(cfg.instance_id);
            if (instanceId.value === cfg.instance_id) {
                applyLoadedNetworkConfig(refreshed);
            }
        } catch (refreshError) {
            console.warn('failed to refresh network config after start', refreshError);
            if (instanceId.value === cfg.instance_id) {
                applyLoadedNetworkConfig(cfg);
            }
        }
    } catch (e: any) {
        console.error(e);
        if (!offerReloadOnRevisionConflict(e)) {
            toast.add({
                severity: 'error',
                summary: t('web.common.error'),
                detail: t('web.device_management.start_failed') + ': ' + errorDetail(e),
                life: TOAST_LIFE.error,
            });
        }
        return;
    }

    emits('update');
    if (isConfigMode.value) {
        isEditingNetwork.value = true;
    } else {
        isEditingNetwork.value = false;
    }
}

function isNetworkConfigArg(value: unknown): value is NetworkTypes.NetworkConfig {
    return !!value
        && typeof value === 'object'
        && typeof (value as NetworkTypes.NetworkConfig).instance_id === 'string';
}

/** Running-state re-apply rebuilds the instance; confirm before brief disconnect. */
const confirmRunNetwork = (config?: NetworkTypes.NetworkConfig | Event) => {
    // Footer @click must use confirmRunNetwork() — a bare handler would pass the MouseEvent.
    const cfg = isNetworkConfigArg(config) ? config : currentNetworkConfig.value;
    if (!cfg) {
        return;
    }
    const rerunning = !networkIsDisabled.value && selectedNetworkRunning.value;
    confirm.require({
        message: t(
            rerunning
                ? 'web.device_management.confirm_rerun_network'
                : 'web.device_management.confirm_start_overwrite',
        ),
        header: t('run_network'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('run_network'),
            severity: 'success',
        },
        accept: () => { void saveAndRunNewNetwork(cfg) },
    });
}

const saveNetworkConfig = async () => {
    if (!currentNetworkConfig.value || savingConfig.value || !canSaveConfig.value) {
        return;
    }
    savingConfig.value = true;
    try {
        await props.api.save_config(currentNetworkConfig.value);

        delete networkMetaCache.value[currentNetworkConfig.value.instance_id];
        await loadNetworkMetas([currentNetworkConfig.value.instance_id]);

        applyLoadedNetworkConfig(currentNetworkConfig.value);
    } catch (e: any) {
        console.error(e);
        if (!offerReloadOnRevisionConflict(e)) {
            toast.add({
                severity: 'error',
                summary: t('web.common.error'),
                detail: t('web.device_management.save_failed') + ': ' + errorDetail(e),
                life: TOAST_LIFE.error,
            });
        }
    } finally {
        savingConfig.value = false;
    }
}
const newNetwork = async () => {
    const newNetworkConfig = props.newConfigGenerator?.() ?? NetworkTypes.DEFAULT_NETWORK_CONFIG();
    try {
        await props.api.save_config(newNetworkConfig);
        setSelectedInstanceId(newNetworkConfig.instance_id);
        applyLoadedNetworkConfig(newNetworkConfig);
        isEditingNetwork.value = true;
        await loadNetworkInstanceIds();
    } catch (e) {
        console.error(e);
        if (!offerReloadOnRevisionConflict(e)) {
            toast.add({
                severity: 'error',
                summary: t('web.common.error'),
                detail: t('web.device_management.save_failed') + ': ' + errorDetail(e),
                life: TOAST_LIFE.error,
            });
        }
    }
}

/** Discard unsaved draft by reloading last saved config. */
const discardConfigChanges = async () => {
    if (!isConfigDirty.value) {
        return;
    }
    try {
        await loadCurrentNetworkConfig({ force: true });
    } catch (e: any) {
        console.error(e);
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: t('web.device_management.load_config_failed') + ': ' + errorDetail(e),
            life: TOAST_LIFE.error,
        });
    }
}

/** Combined：退出配置，回到运行页（运行中状态或已停止空态）。 */
const exitEditNetwork = () => {
    if (isCombinedMode.value && isEditingNetwork.value) {
        isEditingNetwork.value = false;
    }
}

const editNetwork = async () => {
    if (!instanceId.value) {
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: t('web.device_management.no_network_selected'),
            life: TOAST_LIFE.error,
        });
        return;
    }

    try {
        const ret = await props.api.get_network_config(instanceId.value!);
        applyLoadedNetworkConfig(ret);
        isEditingNetwork.value = true;
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: t('web.common.error'), detail: t('web.device_management.save_failed') + ': ' + errorDetail(e), life: TOAST_LIFE.error });
        return;
    }
}

const loadNetworkInstanceIds = async () => {
    listInstanceIdResponse.value = await props.api.list_network_instance_ids();
}

const loadCurrentNetworkInfo = async () => {
    const selected = selectedInstanceId.value?.uuid;
    if (!selected) {
        curNetworkInfo.value = null;
        return;
    }
    if (!needShowNetworkStatus.value) {
        curNetworkInfo.value = null;
        return;
    }
    if (curNetworkInfo.value?.instance_id !== selected) {
        curNetworkInfo.value = null;
    }

    let network_info = await props.api.get_network_info(selected);
    if (selectedInstanceId.value?.uuid !== selected) {
        return;
    }

    if (!network_info) {
        curNetworkInfo.value = {
            instance_id: selected,
            running: false,
            error_msg: t('web.device_management.network_info_unavailable'),
        } as NetworkTypes.NetworkInstance;
        return;
    }

    curNetworkInfo.value = {
        instance_id: selected,
        running: network_info?.running ?? false,
        error_msg: network_info?.error_msg ?? '',
        detail: network_info,
    } as NetworkTypes.NetworkInstance;
}

const exportConfig = async () => {
    if (!instanceId.value) {
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: t('web.device_management.no_network_selected'),
            life: TOAST_LIFE.error,
        });
        return;
    }

    const targetInstanceId = instanceId.value;
    try {
        const { instance_id, ...networkConfig } = await props.api.get_network_config(targetInstanceId);
        let { toml_config: tomlConfig, error } = await props.api.generate_config(networkConfig as NetworkTypes.NetworkConfig);
        if (error) {
            throw { response: { data: { message: typeof error === 'string' ? error : String(error) } } };
        }
        if (instanceId.value !== targetInstanceId) {
            return;
        }
        exportTomlFile(tomlConfig ?? '', targetInstanceId + '.toml');
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: t('web.common.error'), detail: t('web.device_management.export_config') + ': ' + errorDetail(e), life: TOAST_LIFE.error });
        return;
    }
}

const importConfig = () => {
    configFile.value.click();
}

const handleFileUpload = (event: Event) => {
    const files = (event.target as HTMLInputElement).files;
    const file = files ? files[0] : null;
    if (!file) return;
    const targetInstanceId = currentNetworkConfig.value?.instance_id ?? instanceId.value;
    const reader = new FileReader();
    reader.onload = async (e) => {
        try {
            let tomlConfig = e.target?.result?.toString();
            if (!tomlConfig) return;
            const resp = await props.api.parse_config(tomlConfig);
            if (resp.error) {
                throw {
                    response: {
                        data: {
                            message: typeof resp.error === 'string' ? resp.error : String(resp.error),
                        },
                    },
                };
            }

            const config = resp.config;
            if (!config) return;
            if (targetInstanceId && instanceId.value !== targetInstanceId) {
                return;
            }

            config.instance_id = targetInstanceId ?? config.instance_id;
            currentNetworkConfig.value = config;
            toast.add({
                severity: 'success',
                summary: t('web.common.success'),
                detail: t('web.device_management.import_config_success'),
                life: TOAST_LIFE.success,
            });
        } catch (error) {
            toast.add({
                severity: 'error',
                summary: t('web.common.error'),
                detail: t('web.device_management.import_config_failed') + ': ' + errorDetail(error),
                life: TOAST_LIFE.error,
            });
        }
        configFile.value.value = null;
    }
    reader.readAsText(file);
}

const exportTomlFile = (context: string, name: string) => {
    let url = window.URL.createObjectURL(new Blob([context], { type: 'application/toml' }));
    let link = document.createElement('a');
    link.style.display = 'none';
    link.href = url;
    link.setAttribute('download', name);
    document.body.appendChild(link);
    link.click();

    document.body.removeChild(link);
    window.URL.revokeObjectURL(url);
}

const generateConfig = async (config: NetworkTypes.NetworkConfig): Promise<string> => {
    let { toml_config: tomlConfig, error } = await props.api.generate_config(config);
    if (error) {
        throw error;
    }
    return tomlConfig ?? '';
}

const syncTomlConfig = async (tomlConfig: string): Promise<void> => {
    const targetInstanceId = currentNetworkConfig.value?.instance_id ?? instanceId.value;
    let resp = await props.api.parse_config(tomlConfig);
    if (resp.error) {
        throw resp.error;
    };
    const config = resp.config;
    if (!config) {
        throw new Error("Parsed config is empty");
    }
    if (targetInstanceId && instanceId.value !== targetInstanceId) {
        throw new Error('Network selection changed while importing configuration');
    }
    config.instance_id = targetInstanceId ?? config.instance_id;
    currentNetworkConfig.value = config;
}

/**
 * GUI combined 运行页「节点配置」：运行中或已停止（只读预览上）均可进入编辑。
 */
const showCombinedEditEntry = computed(() =>
    isCombinedMode.value
    && !isEditingNetwork.value
    && !!selectedInstanceId.value
    && currentNetworkControl.editable.value
    && (needShowNetworkStatus.value || showStatusDisabledPanel.value)
);

/**
 * 取消编辑 + 保存：仅配置页且脏时成对显示。
 */
const showSaveInFooter = computed(() =>
    showConfigPanel.value && !!currentNetworkConfig.value && isConfigDirty.value,
);

const showDiscardChanges = computed(() => showSaveInFooter.value);

/**
 * Combined 配置页干净：返回运行页（运行中或已停止）。
 */
const showExitEdit = computed(() =>
    isCombinedMode.value
    && isEditingNetwork.value
    && showConfigPanel.value
    && !!currentNetworkConfig.value
    && !isConfigDirty.value,
);

const showCombinedNavZone = computed(() =>
    showCombinedEditEntry.value
    || showDiscardChanges.value
    || showSaveInFooter.value
    || showExitEdit.value
);

/** 启停只在运行页；配置页不挂「运行网络」。 */
const stickyFooterPrimary = computed(() => {
    if (showStatusDisabledPanel.value) {
        return 'start' as const;
    }
    if (needShowNetworkStatus.value) {
        return 'stop' as const;
    }
    return 'none' as const;
});

const loggingDialogVisible = ref(false)

const showLoggingSettings = computed(() =>
    !!props.fullPage
    && typeof props.api.get_logger_level === 'function'
    && typeof props.api.set_logger_level === 'function',
)

const loggingApi = computed<LoggingSettingsApi>(() => ({
    remoteOnly: typeof props.api.list_log_files !== 'function',
    getLoggerLevel: props.api.get_logger_level?.bind(props.api),
    setLoggerLevel: (level: string) => props.api.set_logger_level!(level),
    getLogDir: props.api.get_log_dir?.bind(props.api),
    listLogFiles: props.api.list_log_files?.bind(props.api),
    readLogFile: props.api.read_log_file?.bind(props.api),
}))

const showStickyFooter = computed(() =>
    showLeaveInFooter.value || stickyFooterPrimary.value !== 'none' || isCombinedMode.value || showLoggingSettings.value
);

let periodFunc = new Utils.PeriodicTask(async () => {
    if (props.pauseAutoRefresh) {
        return;
    }
    try {
        pollTick += 1
        const jobs: Array<Promise<unknown>> = []
        const shouldListIds = !isConfigMode.value
            || pollTick === 1
            || pollTick % CONFIG_MODE_LIST_POLL_EVERY === 0
        if (shouldListIds) {
            jobs.push(loadNetworkInstanceIds())
        }
        // Config page does not show Status; skip info poll (needShowNetworkStatus is false anyway).
        if (!isConfigMode.value) {
            jobs.push(loadCurrentNetworkInfo())
        }
        // P1-1: while dirty, periodically probe whether the server baseline moved.
        if (
            isConfigDirty.value
            && showConfigPanel.value
            && (pollTick === 1 || pollTick % CONFIG_MODE_LIST_POLL_EVERY === 0)
        ) {
            jobs.push(checkServerConfigStaleWhileDirty())
        }
        if (jobs.length) {
            await Promise.all(jobs)
        }
    } catch (e) {
        console.debug(e);
    }
}, STATUS_POLL_MS);

onMounted(async () => {
    managementAlive = true;
    periodFunc.start();
});

onUnmounted(() => {
    managementAlive = false;
    ++configLoadGeneration; // invalidate in-flight load/probe apply
    periodFunc.stop();
});

</script>


<template>
    <div class="device-management" :class="{ 'device-management--page': fullPage || !!drawerClose }">
        <input type="file" @change="handleFileUpload" class="hidden" accept="application/toml" ref="configFile" />

        <!-- ????????????????????? -->
        <div class="network-header">
            <h1 v-if="pageTitle" class="et-page-title network-page-title">{{ pageTitle }}</h1>
            <div class="network-header-main flex flex-row justify-between items-center gap-2">
                <!-- ?????? -->
                <div class="flex-1 min-w-0">
                    <Select :modelValue="selectedInstanceId" :options="instanceList" optionLabel="uuid"
                        class="w-full network-instance-select"
                        inputId="dd-inst-id" :placeholder="t('web.device_management.select_network')"
                        overlay-class="network-select-overlay"
                        @update:modelValue="onSelectInstanceId"
                        :pt="{
                            root: {
                                class: [
                                    'network-select-container',
                                    {
                                        'network-instance-select--running': selectedNetworkRunning,
                                        'network-instance-select--stopped': selectedNetworkStopped,
                                    },
                                ],
                            },
                            overlay: { class: 'network-select-overlay' },
                        }"
                        :virtualScrollerOptions="{
                            lazy: true,
                            onLazyLoad: onLazyLoadNetworkMetas,
                            itemSize: 60,
                            delay: 50
                        }">
                            <template #value="slotProps">
                                <div v-if="slotProps.value" class="flex items-center min-w-0 gap-2">
                                    <span class="truncate block min-w-0 flex-1 network-instance-label">
                                        <span v-if="slotProps.value.meta">
                                            {{ slotProps.value.meta.network_name }} ({{ slotProps.value.uuid }})
                                        </span>
                                        <span v-else>{{ slotProps.value.uuid }}</span>
                                    </span>
                                    <Tag v-if="isConfigDirty && showConfigPanel" class="network-dirty-tag leading-3 shrink-0"
                                        severity="warn"
                                        :value="t('web.device_management.unsaved_changes')" />
                                    <Tag class="network-status-tag leading-3 shrink-0"
                                        :severity="isRunning(slotProps.value.uuid) ? 'success' : 'danger'"
                                        :value="t(isRunning(slotProps.value.uuid) ? 'network_running' : 'network_stopped')" />
                                </div>
                                <span v-else class="network-select-placeholder">{{ slotProps.placeholder }}</span>
                            </template>
                            <template #option="slotProps">
                                <div class="flex flex-col items-start content-center max-w-full">
                                    <div class="flex items-center min-w-0">
                                        <div class="mr-4 min-w-0 flex-1">
                                            <span class="truncate block">{{ t('network_name') }}: {{
                                                slotProps.option.meta?.network_name ?? slotProps.option.uuid }}</span>
                                        </div>
                                        <Tag class="network-status-tag my-auto leading-3 shrink-0"
                                            :severity="isRunning(slotProps.option.uuid) ? 'success' : 'danger'"
                                            :value="t(isRunning(slotProps.option.uuid) ? 'network_running' : 'network_stopped')" />
                                    </div>
                                    <div class="max-w-full overflow-hidden text-ellipsis text-muted-color">
                                        {{ slotProps.option.uuid }}
                                    </div>
                                </div>
                            </template>
                        </Select>
                </div>
            </div>
        </div>

        <!-- ?????/????? -->
        <div class="network-content">
            <template v-if="showStatusDisabledPanel">
                <Config
                    v-if="statusDisabledConfigPreview"
                    :cur-network="statusDisabledConfigPreview"
                    :config-invalid="false"
                    :hide-run-button="true"
                    :read-only="true"
                />
                <Message v-else severity="info" class="mb-0">
                    {{ t('web.device_management.loading_network_configuration') }}
                </Message>
            </template>

            <template v-else-if="showConfigPanel && currentNetworkConfig">
                <Config :cur-network="currentNetworkConfig" :config-invalid="false"
                    :hide-run-button="true" @run-network="confirmRunNetwork">
                    <template #config-toolbar>
                        <div class="config-toolbar">
                            <div class="toolbar-zone">
                                <span class="toolbar-zone-label">{{ t('web.device_management.toolbar_config_files') }}</span>
                                <div class="toolbar-zone-actions">
                                    <Button class="config-toolbar-btn" @click="showConfigEditDialog = true" icon="pi pi-file-edit"
                                        :label="t('web.device_management.edit_as_file')" iconPos="left" severity="secondary"
                                        outlined
                                        v-tooltip.bottom="t('web.device_management.edit_as_file_tip')" />
                                    <Button class="config-toolbar-btn" @click="importConfig" icon="pi pi-upload"
                                        :label="t('web.device_management.import_config')" iconPos="left" severity="secondary"
                                        outlined
                                        v-tooltip.bottom="t('web.device_management.import_config_tip')" />
                                    <Button v-if="selectedInstanceId" class="config-toolbar-btn" @click="exportConfig" icon="pi pi-download"
                                        :label="t('web.device_management.export_config')" iconPos="left" severity="secondary"
                                        outlined
                                        v-tooltip.bottom="t('web.device_management.export_config_tip')" />
                                </div>
                            </div>
                            <div class="toolbar-zone toolbar-zone--network">
                                <span class="toolbar-zone-label">{{ t('web.device_management.toolbar_network') }}</span>
                                <div class="toolbar-zone-actions">
                                    <Button class="config-toolbar-btn" @click="newNetwork" icon="pi pi-plus"
                                        :label="t('web.device_management.add_network')" iconPos="left" severity="success"
                                        v-tooltip.bottom="t('web.device_management.add_network_tip')" />
                                    <Button v-if="selectedInstanceId && currentNetworkControl.deletable.value"
                                        class="config-toolbar-btn" @click="confirmDeleteNetwork" icon="pi pi-trash"
                                        :label="t('web.device_management.delete_network')" iconPos="left" severity="danger"
                                        outlined
                                        v-tooltip.bottom="t('web.device_management.delete_network_tip')" />
                                </div>
                            </div>
                        </div>
                    </template>
                </Config>
            </template>
            <Message v-else-if="showConfigPanel" severity="info" class="mb-0">
                {{ t('web.device_management.loading_network_configuration') }}
            </Message>

            <template v-else-if="needShowNetworkStatus">
                <Status v-if="curNetworkInfo && curNetworkInfo.error_msg === ''" v-bind:cur-network-inst="curNetworkInfo"
                    :api="api" class="mb-0 status-fill-host" />
                <Message v-else-if="curNetworkInfo?.error_msg" severity="error" class="mb-0">{{
                    curNetworkInfo.error_msg }}</Message>
                <Message v-else severity="info" class="mb-0">{{ t('web.device_management.loading_network_status') }}
                </Message>
            </template>

            <div v-else class="empty-state flex flex-col items-center py-6">
                <i class="pi pi-sitemap text-4xl text-secondary mb-3 opacity-50"></i>
                <div class="text-lg text-center font-medium mb-2">{{ t('web.device_management.no_network_selected') }}
                </div>
                <p class="text-secondary text-center mb-4 max-w-md">
                    {{ isStatusMode
                        ? t('web.device_management.select_network_for_status')
                        : t('web.device_management.select_existing_network_or_create_new') }}
                </p>
                <Button v-if="!isStatusMode" @click="newNetwork"
                    :label="t('web.device_management.add_network')" icon="pi pi-plus" iconPos="left"
                    v-tooltip.top="t('web.device_management.add_network_tip')" />
                <Button v-else @click="requestSwitchMode('config')"
                    :label="t('web.device_management.switch_to_config')" icon="pi pi-cog" iconPos="left"
                    severity="secondary"
                    v-tooltip.top="t('web.device_management.switch_to_config_tip')" />
            </div>
        </div>

        <div v-if="showStickyFooter" class="network-sticky-footer">
            <Button v-if="showLoggingSettings" :label="t('logging')" icon="pi pi-file" severity="secondary"
                class="network-footer-btn network-footer-btn--muted" @click="loggingDialogVisible = true" />
            <!-- GUI 额外按钮（系统设置等）放最左，窄屏单行时更易点到 -->
            <slot name="footer-extra" />
            <Button v-if="showLeaveInFooter" @click="requestLeave" :label="leaveLabel" severity="secondary"
                :icon="leaveIcon" iconPos="left" class="network-footer-btn network-footer-btn--muted"
                v-tooltip.top="leaveTooltip" />
            <!-- 左区：节点配置入口 / 前往状态 / 取消编辑 / 保存；右区：主操作 -->
            <div v-if="showCombinedNavZone || isConfigMode || isStatusMode" class="footer-zone">
                <Button v-if="showCombinedEditEntry" severity="secondary"
                    :label="t('web.device_management.switch_to_config')"
                    class="network-footer-btn network-footer-btn--muted" @click="editNetwork"
                    v-tooltip.top="t('web.device_management.switch_to_config_tip')" />
                <Button v-if="showExitEdit" severity="secondary"
                    :label="t('web.device_management.switch_to_status')"
                    class="network-footer-btn network-footer-btn--muted" @click="exitEditNetwork"
                    v-tooltip.top="t('web.device_management.switch_to_status_tip')" />
                <Button v-if="showDiscardChanges" severity="secondary"
                    :label="t('web.device_management.discard_changes')"
                    class="network-footer-btn network-footer-btn--muted" @click="discardConfigChanges"
                    v-tooltip.top="t('web.device_management.discard_changes_tip')" />
                <Button v-if="showSaveInFooter" severity="success"
                    :label="t('web.device_management.save_config')"
                    class="network-footer-btn"
                    :disabled="!canSaveConfig || savingConfig"
                    @click="saveNetworkConfig"
                    v-tooltip.top="saveConfigTooltip" />
                <Button v-if="isConfigMode" severity="secondary"
                    :label="t('web.device_management.switch_to_status')"
                    class="network-footer-btn network-footer-btn--accent" @click="requestSwitchMode('status')"
                    v-tooltip.top="t('web.device_management.switch_to_status_tip')" />
                <Button v-else-if="isStatusMode" severity="secondary"
                    :label="t('web.device_management.switch_to_config')"
                    class="network-footer-btn network-footer-btn--accent" @click="requestSwitchMode('config')"
                    v-tooltip.top="t('web.device_management.switch_to_config_tip')" />
            </div>
            <div class="footer-zone footer-zone--primary">
                <Button v-if="stickyFooterPrimary === 'start'" @click="startNetwork()"
                    :disabled="!currentNetworkControl.deletable.value"
                    :label="t('web.device_management.start_network')"
                    severity="success" class="network-footer-btn"
                    v-tooltip.top="t('web.device_management.start_network_tip')" />
                <Button v-else-if="stickyFooterPrimary === 'stop'" @click="stopNetwork()"
                    :disabled="!currentNetworkControl.deletable.value"
                    :label="t('web.device_management.disable_network')" severity="danger"
                    class="network-footer-btn network-footer-btn--danger"
                    v-tooltip.top="t('web.device_management.disable_network_tip')" />
            </div>
        </div>

        <ConfigEditDialog v-model:visible="showConfigEditDialog" :cur-network="currentNetworkConfig"
            :generate-config="generateConfig" :save-config="syncTomlConfig" />
        <LoggingSettingsDialog v-if="showLoggingSettings" v-model:visible="loggingDialogVisible" :api="loggingApi" />
    </div>
</template>

<style scoped>
.device-management {
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
    gap: 0.25rem;
    overflow: hidden;
}

.device-management--page {
    height: 100%;
    max-height: 100%;
    overflow: hidden;
}

.network-header {
    flex-shrink: 0;
    z-index: 20;
    background: var(--surface-card, #ffffff) !important;
    border: 1px solid var(--et-border-color, #e2e8f0);
    border-radius: var(--et-radius, 0.75rem);
    box-shadow: var(--et-shadow-card, none);
    padding: 0.75rem 1rem !important;
    margin: 0 !important;
}

.network-page-title {
    position: relative;
    margin: 0 0 0.55rem;
    padding-left: 0.75rem;
    font-size: var(--et-fs-page-title, 1.25rem);
    font-weight: 700;
    line-height: 1.3;
    letter-spacing: -0.02em;
    color: var(--text-color, #1e293b);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.network-page-title::before {
    content: "";
    position: absolute;
    left: 0;
    top: 0.2rem;
    bottom: 0.2rem;
    width: 0.25rem;
    border-radius: 999px;
    background: var(--primary-color, var(--et-primary, #0ea5e9));
}

.network-header-main {
    min-width: 0;
}

.network-instance-label {
    font-size: var(--et-fs-body, 0.875rem);
    color: var(--text-color-secondary, #64748b);
}

.network-select-placeholder {
    color: var(--et-placeholder-color, #a8b5c5);
}

.network-content {
    flex: 1 1 auto;
    display: flex;
    flex-direction: column;
    overflow-y: auto;
    min-height: 0;
    padding: 0.65rem 0.75rem !important;
    background: var(--surface-card, #ffffff) !important;
    border: 1px solid var(--et-border-color, #e2e8f0);
    border-radius: var(--et-radius, 0.75rem);
    box-shadow: var(--et-shadow-card, none) !important;
}

/* Status 列表占满内容区，便于展开面板吃掉剩余高度 */
.network-content > .status-fill-host {
    flex: 1 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
}

.network-sticky-footer {
    flex-shrink: 0;
    z-index: 20;
    margin-top: 0;
    padding: 0.55rem 0.75rem;
    background: var(--surface-card, #ffffff);
    border: 1px solid var(--et-border-color, #e2e8f0);
    border-radius: var(--et-radius, 0.75rem);
    box-shadow: var(--et-shadow-card, none);
    display: flex;
    /* 底部主操作尽量始终单行，避免 2/3 按钮折行看起来散 */
    flex-wrap: nowrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
}

.footer-zone {
    display: flex;
    flex-wrap: nowrap;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
    /* 与主区/插槽按钮一起平分整行（窄屏四钮等宽占满） */
    flex: 1 1 0;
}

.footer-zone--primary {
    justify-content: flex-end;
    margin-left: auto;
    flex: 1 1 0;
}

/* footer-extra 插槽按钮（系统设置等）同样参与平分 */
.network-sticky-footer > :deep(.network-footer-btn.p-button) {
    flex: 1 1 0;
    min-width: 0;
}

@media (max-width: 640px) {
    /* 窄屏藏底部按钮图标，只留短文案，四钮才排得下 */
    .network-sticky-footer :deep(.network-footer-btn.p-button .p-button-icon) {
        display: none;
    }
}

/* 同高同圆角；宽度随可用空间均分，保证一行排得下 */
:deep(.network-footer-btn.p-button) {
    flex: 1 1 0;
    width: auto;
    min-width: 0;
    max-width: var(--et-btn-w, 11.5rem);
    height: var(--et-btn, 2.5rem) !important;
    padding: 0 0.65rem !important;
    font-size: var(--et-fs-body, 0.875rem) !important;
    font-weight: 600 !important;
    border-radius: var(--et-radius, 0.75rem) !important;
    box-sizing: border-box;
    justify-content: center;
}

:deep(.network-footer-btn.p-button .p-button-label) {
    white-space: nowrap;
    overflow: visible;
    text-overflow: clip;
}

:deep(.network-footer-btn--muted.p-button),
:deep(.network-footer-btn.p-button.p-button-secondary) {
    background: var(--surface-100, #f1f5f9) !important;
    border: 1px solid var(--et-border-color, #e2e8f0) !important;
    color: var(--text-color, #1e293b) !important;
}

:deep(.network-footer-btn--muted.p-button:hover:not(:disabled)),
:deep(.network-footer-btn.p-button.p-button-secondary:hover:not(:disabled)) {
    background: var(--surface-200, #e2e8f0) !important;
    border-color: color-mix(in srgb, var(--primary-color, #0ea5e9) 35%, var(--et-border-color, #e2e8f0)) !important;
    color: var(--text-color, #1e293b) !important;
}

:deep(.network-footer-btn--accent.p-button) {
    background: color-mix(in srgb, var(--primary-color, #0ea5e9) 12%, #ffffff) !important;
    border: 1px solid color-mix(in srgb, var(--primary-color, #0ea5e9) 32%, var(--et-border-color, #e2e8f0)) !important;
    color: var(--primary-color, #0284c7) !important;
}

:deep(.network-footer-btn--accent.p-button:hover:not(:disabled)) {
    background: color-mix(in srgb, var(--primary-color, #0ea5e9) 18%, #ffffff) !important;
}

:deep(.network-footer-btn--danger.p-button),
:deep(.network-footer-btn.p-button-danger) {
    background: color-mix(in srgb, #ef4444 12%, #ffffff) !important;
    border: 1px solid color-mix(in srgb, #ef4444 35%, #e2e8f0) !important;
    color: #b91c1c !important;
}

:deep(.network-footer-btn--danger.p-button:hover:not(:disabled)),
:deep(.network-footer-btn.p-button-danger:hover:not(:disabled)) {
    background: color-mix(in srgb, #ef4444 20%, #ffffff) !important;
}

:deep(.network-footer-btn.p-button-success) {
    background: color-mix(in srgb, #10b981 14%, #ffffff) !important;
    border: 1px solid color-mix(in srgb, #10b981 38%, #e2e8f0) !important;
    color: #047857 !important;
}

:deep(.network-footer-btn.p-button-success:hover:not(:disabled)) {
    background: color-mix(in srgb, #10b981 22%, #ffffff) !important;
}

:deep(.header-action-btn.p-button) {
    height: var(--et-btn, 2.5rem) !important;
    min-height: var(--et-btn, 2.5rem) !important;
    padding: 0 0.9rem !important;
    font-size: var(--et-fs-body, 0.875rem) !important;
    font-weight: 600 !important;
    border-radius: var(--et-radius, 0.75rem) !important;
}

:deep(.header-action-btn--icon.p-button),
:deep(.header-action-btn.p-button-icon-only) {
    width: var(--et-btn, 2.5rem) !important;
    min-width: var(--et-btn, 2.5rem) !important;
    padding: 0 !important;
}

:deep(.config-toolbar-btn.p-button) {
    height: var(--et-btn, 2.5rem) !important;
    min-height: var(--et-btn, 2.5rem) !important;
    width: var(--et-btn-w, 10rem);
    min-width: var(--et-btn-w, 10rem);
    padding: 0 0.9rem !important;
    font-size: var(--et-fs-body, 0.875rem) !important;
    font-weight: 600 !important;
    border-radius: var(--et-radius, 0.75rem) !important;
    box-sizing: border-box;
    justify-content: center;
}

:deep(.network-select-container.p-select),
:deep(.network-instance-select.p-select) {
    border: 1px solid var(--et-border-color, #e2e8f0) !important;
    border-radius: calc(var(--et-radius, 0.75rem) - 0.125rem) !important;
    background: #ffffff !important;
    background-color: #ffffff !important;
    transition: background-color 0.15s ease, border-color 0.15s ease, color 0.15s ease;
}

:deep(.network-select-container.p-select:not(.p-disabled):hover),
:deep(.network-instance-select.p-select:not(.p-disabled):hover) {
    border-color: color-mix(in srgb, var(--primary-color, #0ea5e9) 40%, var(--et-border-color, #e2e8f0)) !important;
}

/* ?????????=???????=??? */
:deep(.network-instance-select--running.p-select),
:deep(.network-instance-select.network-instance-select--running) {
    background: #10b981 !important;
    background-color: #10b981 !important;
    border-color: #059669 !important;
    color: #ffffff !important;
}

:deep(.network-instance-select--stopped.p-select),
:deep(.network-instance-select.network-instance-select--stopped) {
    background: #ef4444 !important;
    background-color: #ef4444 !important;
    border-color: #dc2626 !important;
    color: #ffffff !important;
}

:deep(.network-instance-select--running .network-instance-label),
:deep(.network-instance-select--stopped .network-instance-label),
:deep(.network-instance-select--running .p-select-dropdown),
:deep(.network-instance-select--stopped .p-select-dropdown),
:deep(.network-instance-select--running .p-select-dropdown-icon),
:deep(.network-instance-select--stopped .p-select-dropdown-icon),
:deep(.network-instance-select--running .p-select-label),
:deep(.network-instance-select--stopped .p-select-label) {
    color: #ffffff !important;
}

:deep(.network-status-tag.p-tag),
:deep(.network-dirty-tag.p-tag) {
    font-weight: 700 !important;
    letter-spacing: 0.02em;
    padding: 0.2rem 0.55rem !important;
    border: none !important;
}

:deep(.network-dirty-tag.p-tag) {
    background: color-mix(in srgb, #f59e0b 22%, #ffffff) !important;
    color: #b45309 !important;
}

/* ???????????Tag ?????????????? */
:deep(.network-instance-select--running .network-status-tag.p-tag),
:deep(.network-instance-select--stopped .network-status-tag.p-tag) {
    background: transparent !important;
    background-color: transparent !important;
    color: #ffffff !important;
    padding-inline: 0.15rem !important;
}

/* ????????????????? */
:deep(.p-select-overlay .network-status-tag.p-tag-success),
:deep(.network-select-overlay .network-status-tag.p-tag-success) {
    background: #10b981 !important;
    color: #ffffff !important;
}

:deep(.p-select-overlay .network-status-tag.p-tag-danger),
:deep(.network-select-overlay .network-status-tag.p-tag-danger) {
    background: #ef4444 !important;
    color: #ffffff !important;
}

.config-toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 0.5rem 1rem;
    margin: 0;
}

.toolbar-zone {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    min-width: 0;
}

.toolbar-zone-label {
    font-size: var(--et-fs-meta, 0.75rem);
    font-weight: 600;
    color: var(--text-color-secondary, #64748b);
    letter-spacing: 0.02em;
}

.toolbar-zone-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
}

.toolbar-zone--network {
    padding-left: 0.85rem;
    border-left: 1px solid var(--surface-border, #e2e8f0);
}

/*
 * ?????640px??????????????????????????????
 *
 * ???????????? `?????`?5 ???????
 *   ?????? ? ?? ? 45px?et-main-content 0.75rem?2 + ?? 0.6rem?2 + ???
 *   360px ? ? ? 315px ??
 *   ? ????5?14(?) + 16(??) + 8(??) + 16(???) ? 110px ? 315/110 = 2.9????? 2~3 ?
 *   ? ??? + 13px ??5?13 + 8(???) ? 73px ? 315/73 = 4.3?????? 4 ?
 * ?????????????? 0.8125rem????? 4 ??
 *
 * ???????????? 3~4 ? ? ???????? 1~2 ? ? ??????? 3 ? ? ?? 3 ??
 * ? auto-fit + minmax(0, 1fr) ????????????????????????? 3+1 ?????
 */
@media (max-width: 640px) {
    .toolbar-zone,
    .toolbar-zone--network {
        width: 100%;
        padding-left: 0;
        border-left: none;
    }

    .network-page-title {
        font-size: 1.05rem;
        margin-bottom: 0.4rem;
    }

    .network-header {
        padding: 0.6rem 0.7rem !important;
    }

    .toolbar-zone--network {
        padding-top: 0.5rem;
        border-top: 1px solid var(--surface-border, #e2e8f0);
    }

    .toolbar-zone-actions {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(0, 1fr));
        /* ????????????????????? */
        align-items: stretch;
        gap: 0.3rem;
    }

    .network-sticky-footer {
        /* 窄屏也强制单行等分，避免折成两行 */
        display: flex;
        flex-wrap: nowrap;
        align-items: stretch;
        gap: 0.3rem;
    }

    /* 展开子区，让按钮都成为 footer 的直接 flex 子项以便均分 */
    .footer-zone,
    .footer-zone--primary {
        display: contents;
    }

    :deep(.network-footer-btn.p-button),
    :deep(.config-toolbar-btn.p-button) {
        flex: 1 1 0;
        min-width: 0;
        max-width: none;
        width: auto;
        height: auto !important;
        min-height: var(--et-btn, 2.5rem) !important;
        padding: 0.3rem !important;
        font-size: 0.75rem !important;
    }

    :deep(.config-toolbar-btn.p-button .p-button-icon),
    :deep(.network-footer-btn.p-button .p-button-icon) {
        display: none;
    }

    :deep(.config-toolbar-btn.p-button .p-button-label),
    :deep(.network-footer-btn.p-button .p-button-label) {
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
        line-height: 1.15;
    }
}

/* ???? */
.button-container {
    gap: 0.5rem;
    align-items: center;
}

/* ?????? */
:deep(.p-menu) {
    min-width: 12rem;
    box-shadow: 0 0.5rem 1rem rgba(0, 0, 0, 0.15);
    padding: 0.25rem;
}

:deep(.p-menu .p-menuitem) {
    border-radius: 0.25rem;
}

:deep(.p-menu .p-menuitem-link) {
    padding: 0.65rem 1rem;
    font-size: 0.9rem;
}

:deep(.p-menu .p-menuitem-icon) {
    margin-right: 0.75rem;
}

/* ???????????????????? */
:deep(.header-action-btn.p-button-icon-only .p-button-icon) {
    font-size: 1rem;
    margin: 0;
}

/* ???????? */
:deep(.network-select-container) {
    max-width: 100%;
}

/* Dark mode adaptations */
:deep(.bg-surface-50) {
    background-color: var(--surface-50, #f8fafc);
}

:deep(.bg-surface-0) {
    background-color: var(--surface-card, #ffffff);
}

:deep(.text-primary) {
    color: var(--primary-color, var(--et-primary, #0ea5e9));
}

:deep(.text-secondary) {
    color: var(--text-color-secondary, #64748b);
}

@media (prefers-color-scheme: dark) {
    :deep(.bg-surface-50) {
        background-color: var(--surface-ground, #0f172a);
    }

    :deep(.bg-surface-0) {
        background-color: var(--surface-card, #1e293b);
    }

    .network-header,
    .network-content,
    .network-sticky-footer {
        background: var(--surface-card, #1e293b) !important;
        border-color: var(--surface-border, #334155);
        box-shadow: none !important;
    }
}

/*
 * ?? / ????641?768px????????? 8.5rem??????????????????
 * ????? 4 ???????
 * ???? min-width: 641px ?? ????? ?640px ???????????? min-width: 0 ???
 */
@media (min-width: 641px) and (max-width: 768px) {
    .network-header {
        padding: 0.5rem 0.6rem;
    }

    .network-content {
        padding: 0.5rem 0.6rem !important;
    }

    :deep(.network-footer-btn.p-button),
    :deep(.config-toolbar-btn.p-button) {
        min-width: 8.5rem;
    }
}
</style>
