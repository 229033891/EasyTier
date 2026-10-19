<script setup lang="ts">
import { Button, Message, Select, Tag, useConfirm, useToast, type VirtualScrollerLazyEvent } from 'primevue';
import { TOAST_LIFE, addThrottledToast, createToastRepeatGate } from '../modules/toast'
import { computed, onMounted, onUnmounted, ref, toRaw, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import * as Api from '../modules/api';
import {
    captureNormalizedDirtySnapshot,
    configDirtySnapshot,
    isConfigSnapshotDirty,
} from '../modules/config-dirty';
import {
    classifyResidual,
    loadIgnoredResiduals,
    persistIgnoredResiduals,
    shouldAutoStartAfterDisabledSave,
    type ResidualKind,
} from '../modules/config-residual';
import * as Utils from '../modules/utils';
import { collectNetworkInfoSingleFlight } from '../modules/collectNetworkInfoSingleFlight';
import { vipReachabilityScore } from '../modules/statusDisplay';
import * as NetworkTypes from '../types/network';
import LoggingSettingsDialog from './LoggingSettingsDialog.vue';
import type { LoggingSettingsApi } from '../modules/logging';

const { t } = useI18n()

const props = defineProps<{
    api: Api.RemoteClient;
    newConfigGenerator?: () => NetworkTypes.NetworkConfig;
    pauseAutoRefresh?: boolean;
    /** Web Drawer / GUI 抽屉关闭；全页模式用页头返回，勿与之重复 */
    drawerClose?: () => void;
    /** 离开按钮文案，默认 t('close') */
    leaveButtonLabel?: string;
    /** 离开按钮悬停说明 */
    leaveButtonTooltip?: string;
    /** 离开按钮图标，默认 pi-times */
    leaveButtonIcon?: string;
    /** Web 独立全页管理（用页头返回，底栏不再放返回） */
    fullPage?: boolean;
    /**
     * Web 设备列表分流：status=查看/启停；config=编辑/新建。
     * 不传则保持 GUI 原「状态+配置」合一行为。
     */
    mode?: 'status' | 'config';
    /** 全页标题，放在网络选择栏内，避免页头再空出一截 */
    pageTitle?: string;
    /** Heartbeat fields for B6 DNS coverage badge (device management page). */
    deviceOsType?: string;
    deviceEasytierVersion?: string;
    deviceMagicDnsOsWired?: boolean | null;
    /**
     * Delete authority for Web-source rows (config-delete-authority.md).
     * - `local` (default): this UI may delete any row (subject to file permission).
     * - `console`: Web-source rows are console-managed; gray out local delete.
     * Independent of {@link hostSurface} —do not infer "I am the web console" from this.
     */
    deleteAuthority?: 'local' | 'console';
    /**
     * Which product surface mounts this component. Independent of deleteAuthority.
     * - `web-console`: DeviceManagement —User-source delete shows §5 confirm.
     * - `gui-client`: desktop GUI —never use deleteAuthority alone to mean console.
     */
    hostSurface?: 'web-console' | 'gui-client';
    /**
     * Scope key for ignored residual ids (usually machine_id). Required for
     * console residual ignore persistence; omit →ignore is session-only.
     */
    residualIgnoreScope?: string;
}>();

const isStatusMode = computed(() => props.mode === 'status')
const isConfigMode = computed(() => props.mode === 'config')
/** GUI 等未指定 mode 时的兼容模式 */
const isCombinedMode = computed(() => !props.mode)
const leaveLabel = computed(() => props.leaveButtonLabel || t('close'))
const leaveTooltip = computed(() => props.leaveButtonTooltip || leaveLabel.value)
const leaveIcon = computed(() => props.leaveButtonIcon || 'pi pi-times')
/** 有离开回调时底栏显示返回/关闭（全页与抽屉均用底栏，不再用页头箭头） */
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

/** False after unmount —blocks late probes / confirms on a destroyed instance. */
let managementAlive = true;

function forceReloadLatestConfig() {
    if (!managementAlive) {
        return;
    }
    void loadCurrentNetworkConfig({ force: true, reportFailure: true }).catch((reloadError) => {
        if (!managementAlive) {
            return;
        }
        console.error(reloadError);
        if (!wasConfigLoadFailureReported(reloadError)) {
            toast.add({
                severity: 'error',
                summary: t('web.common.error'),
                detail: t('web.device_management.load_config_failed') + ': ' + errorDetail(reloadError),
                life: TOAST_LIFE.error,
            });
        }
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

/**
 * Roadmap §2.3「最上方（实例态旁）」的 VIP 可达 `[M/N]`。
 *
 * Same denominator as the Status page: the number of links the prober actually
 * probed (`peer_vip_reachability.length`). This view renders no peer table, so
 * there is no row count to compare against — which is why the two disagreeing
 * numbers that motivated the Status-page design never arise here.
 *
 * Only the selected instance has a live `detail` (that is `curNetworkInfo`), so
 * the other rows have no probe data and get no badge — showing `[?/N]` for
 * every unselected instance would be noise, not information.
 */
function vipBadgeFor(instanceId: string): {
    text: string
    severity: 'success' | 'warn' | 'secondary'
    tip: string
} | undefined {
    const info = curNetworkInfo.value
    if (!info || info.instance_id !== instanceId || !info.detail)
        return undefined
    const score = vipReachabilityScore(info.detail.peer_vip_reachability)
    // `hasData === false` means the core did not send the field at all, so N is
    // unknown too — there is no `[?/N]` to show. "Probing disabled" reaches here
    // as explicit `UNKNOWN + reason` rows instead, rendered `[0/N]` in gray.
    if (!score.hasData || score.total === 0)
        return undefined
    const text = `[${score.reachable}/${score.total}]`
    const severity: 'success' | 'warn' | 'secondary' = (
        score.reachable === 0 && score.anyUnknown)
        ? 'secondary'
        : (score.reachable === score.total ? 'success' : 'warn')
    const tip = t('status.vip_reachability_badge_tip', { m: score.reachable, n: score.total })
    return { text, severity, tip }
}

/** Residual badges only on console when list carries DB id sets (P1). */
const residualDetectReady = computed(() =>
    props.hostSurface === 'web-console'
    && Array.isArray(listInstanceIdResponse.value?.db_inst_ids),
);

const residualListSets = computed(() => ({
    runningIds: runningInstanceIds.value,
    dbInstIds: new Set(
        (listInstanceIdResponse.value?.db_inst_ids ?? []).map(Utils.UuidToStr),
    ),
    dbDisabledInstIds: new Set(
        (listInstanceIdResponse.value?.db_disabled_inst_ids ?? []).map(Utils.UuidToStr),
    ),
}));

const ignoredResiduals = ref<Set<string>>(loadIgnoredResiduals(props.residualIgnoreScope));

watch(
    () => props.residualIgnoreScope,
    (scope) => {
        ignoredResiduals.value = loadIgnoredResiduals(scope);
    },
);

const residualKindFor = (id: string | undefined): ResidualKind | null => {
    if (!id || !residualDetectReady.value || ignoredResiduals.value.has(id)) {
        return null;
    }
    return classifyResidual(id, residualListSets.value);
};

const currentResidualKind = computed(() => residualKindFor(instanceId.value));

const residualBadgeLabel = (kind: ResidualKind) =>
    kind === 'device_only'
        ? t('web.device_management.residual_device_only')
        : t('web.device_management.residual_db_disabled_running');

const ignoreCurrentResidual = () => {
    const id = instanceId.value;
    if (!id) {
        return;
    }
    const next = new Set(ignoredResiduals.value);
    next.add(id);
    ignoredResiduals.value = next;
    persistIgnoredResiduals(props.residualIgnoreScope, next);
};

const clearResidualDeviceCopy = () => {
    const kind = currentResidualKind.value;
    const id = instanceId.value;
    if (!kind || !id) {
        return;
    }
    // DB-disabled-but-running: stop device, keep console row (clear device copy).
    if (kind === 'db_disabled_running') {
        void stopNetwork();
        return;
    }
    // Device-only: hard purge (keep=false). Web →light confirm; User →stronger confirm.
    const source = Api.normalizeConfigSource(currentNetworkMeta.value?.source);
    const isUser =
        source === Api.ConfigSource.User
        || source === undefined;
    confirm.require({
        message: t(
            isUser
                ? 'web.device_management.confirm_delete_user_source'
                : 'web.device_management.confirm_clear_web_residual',
        ),
        header: t('web.device_management.clear_device_copy'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.device_management.clear_device_copy'),
            severity: 'danger',
        },
        accept: () => { void doDeleteNetwork() },
    });
};

const takeoverResidualToConsole = () => {
    const kind = currentResidualKind.value;
    if (!kind) {
        return;
    }
    // Already in console as disabled →Start aligns run state.
    if (kind === 'db_disabled_running') {
        startNetwork();
        return;
    }
    // Device-only →Save upserts into console (User→Web confirm inside saveNetworkConfig).
    if (!currentNetworkConfig.value || !canSaveConfig.value) {
        toast.add({
            severity: 'warn',
            summary: t('web.common.warning'),
            detail: saveConfigDisabledReason.value
                || t('web.device_management.save_config_disabled_meta'),
            life: TOAST_LIFE.warn,
        });
        return;
    }
    void saveNetworkConfig();
};

const networkMetaCache = ref<Record<string, Api.NetworkMeta>>({});
const networkMetaLoadFailed = ref<Record<string, boolean>>({});
const loadNetworkMetas = async (instanceIds: string[]) => {
    const missingIds = instanceIds.filter(id => !networkMetaCache.value[id]);

    if (missingIds.length === 0) return;

    try {
        const response = await props.api.get_network_metas(missingIds);
        const metas = response.metas ?? {};
        for (const [id, meta] of Object.entries(metas)) {
            networkMetaCache.value[id] = {
                ...meta,
                source: Api.normalizeConfigSource(meta.source),
            };
        }
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


/**
 * Instance Select label for `network_name`.
 * Prefer the open editor draft so typing / Save-without-restart stays in sync
 * with the form (meta can lag on a still-running instance until the next Run).
 */
function displayNetworkName(uuid: string, meta?: Api.NetworkMeta): string {
    const draft = currentNetworkConfig.value;
    if (
        draft
        && draft.instance_id === uuid
        && showConfigPanel.value
        && typeof draft.network_name === 'string'
        && draft.network_name.length > 0
    ) {
        return draft.network_name;
    }
    return meta?.network_name || uuid;
}

/** Keep meta cache aligned after Save so list/options match storage identity. */
function patchMetaNetworkName(instanceId: string, networkName: string | undefined) {
    if (!instanceId || networkName == null) {
        return;
    }
    const prev = networkMetaCache.value[instanceId];
    networkMetaCache.value[instanceId] = {
        network_name: networkName,
        config_permission: prev?.config_permission ?? 0,
        source: prev?.source,
    };
    updateInstanceList();
}

/** Ownership badge: Web = console; User = local; missing/Unspecified = unknown (always shown). */
type ConfigSourceBadge = {
    kind: 'console' | 'local' | 'pending' | 'unknown';
    label: string;
    tip: string;
    icon: string;
    severity: 'info' | 'secondary' | 'warn';
};

function unknownConfigSourceBadge(): ConfigSourceBadge {
    return {
        kind: 'unknown',
        label: t('web.device_management.config_source_unknown'),
        tip: t('web.device_management.config_source_unknown_tip'),
        icon: 'pi pi-question-circle',
        severity: 'secondary',
    };
}

function configSourceBadgeFor(
    meta: Api.NetworkMeta | undefined,
    opts?: { dirty?: boolean },
): ConfigSourceBadge {
    const source = Api.normalizeConfigSource(meta?.source);
    if (!meta || source === undefined || source === Api.ConfigSource.Unspecified) {
        return unknownConfigSourceBadge();
    }
    if (source === Api.ConfigSource.Web) {
        // Ownership ≠sync success: dirty Web drafts are still console-owned but pending push.
        if (opts?.dirty) {
            return {
                kind: 'pending',
                label: t('web.device_management.config_source_pending_sync'),
                tip: t('web.device_management.config_source_pending_sync_tip'),
                icon: 'pi pi-cloud-upload',
                severity: 'warn',
            };
        }
        return {
            kind: 'console',
            label: t('web.device_management.config_source_console'),
            tip: t('web.device_management.config_source_console_tip'),
            icon: 'pi pi-cloud',
            severity: 'info',
        };
    }
    if (source === Api.ConfigSource.User) {
        return {
            kind: 'local',
            label: t('web.device_management.config_source_local'),
            tip: t('web.device_management.config_source_local_tip'),
            icon: 'pi pi-desktop',
            severity: 'secondary',
        };
    }
    return unknownConfigSourceBadge();
}

const currentConfigSourceBadge = computed(() =>
    configSourceBadgeFor(currentNetworkMeta.value, {
        dirty: isConfigDirty.value && showConfigPanel.value,
    }),
);

/** Per-instance badges for the network Select dropdown (avoids repeated helper calls). */
const configSourceBadgeById = computed(() => {
    const map: Record<string, ConfigSourceBadge> = {};
    for (const inst of instanceList.value) {
        map[inst.uuid] = configSourceBadgeFor(
            networkMetaCache.value[inst.uuid] ?? inst.meta,
        );
    }
    return map;
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

/**
 * Web-source row locked to console when host injects `deleteAuthority: 'console'`.
 * Fail-open while meta is missing (`source` undefined) —button stays enabled until
 * meta loads; GUI backend `remove_network_instance` is the hard gate.
 */
const isConsoleManagedDelete = computed(() => {
    if (props.deleteAuthority !== 'console') {
        return false;
    }
    return Api.normalizeConfigSource(currentNetworkMeta.value?.source) === Api.ConfigSource.Web;
});

const canDeleteNetworkLocally = computed(
    () => currentNetworkControl.deletable.value && !isConsoleManagedDelete.value,
);

const deleteNetworkTooltip = computed(() =>
    isConsoleManagedDelete.value
        ? t('web.device_management.delete_managed_by_console')
        : t('web.device_management.delete_network_tip'),
);

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

/** Status CollectNetworkInfo cadence; 5s leaves BidirectRpc room for config / heartbeat / history (S12). */
const STATUS_POLL_MS = 5000
/** Config editor only needs occasional running/stopped tags; ~15s with STATUS_POLL_MS. */
const CONFIG_MODE_LIST_POLL_EVERY = 3
let pollTick = 0
/** Invalidate in-flight status collect apply on instance switch / unmount (S10). */
let networkInfoLoadGeneration = 0

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
        .map(({ uuid, meta }) => `${uuid}\u0000${meta?.network_name ?? ''}\u0000${meta?.config_permission ?? ''}\u0000${meta?.source ?? ''}`)
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
    // 打开 Drawer 未带 instanceId 时，优先选中运行中实例，否则选第一个
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

/** Programmatic selection (run/new/etc.) —skips dirty confirm. */
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
        if (newVal?.uuid !== oldVal?.uuid) {
            ++networkInfoLoadGeneration
        }
        const loads: Array<Promise<unknown>> = []
        if (newVal?.uuid !== oldVal?.uuid && (networkIsDisabled.value || isEditingNetwork.value || isConfigMode.value)) {
            loads.push(loadCurrentNetworkConfig({ reportFailure: true }))
        } else {
            loads.push(loadCurrentNetworkInfo())
        }

        if (newVal?.uuid && !networkMetaCache.value[newVal.uuid]) {
            loads.push(loadNetworkMetas([newVal.uuid]))
        }
        await Promise.all(loads)
    } catch (e) {
        console.error('Failed to load selected network', e);
        // 配置加载失败已由共享加载路径播报（同一次尝试只一条）；这里只为其它 loader 兜底。
        if (!wasConfigLoadFailureReported(e)) {
            toast.add({
                severity: 'error',
                summary: t('web.common.error'),
                detail: errorDetail(e),
                life: TOAST_LIFE.error,
            });
        }
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
            await loadCurrentNetworkConfig({ reportFailure: true });
        } catch (e) {
            console.error('Failed to load disabled network config', e);
            if (!wasConfigLoadFailureReported(e)) {
                toast.add({
                    severity: 'error',
                    summary: t('web.common.error'),
                    detail: errorDetail(e),
                    life: TOAST_LIFE.error,
                });
            }
        }
    }
});

/** One shared config-load attempt; every caller for the same instance joins it. */
interface ConfigLoadAttempt {
    instanceId: string
    promise: Promise<void>
    /** Set by any joined caller that wants the failure surfaced to the user. */
    reportFailure: boolean
    /** Set when the attempt settles, so a caller joining late can still report. */
    settled: boolean
    rejection: unknown
    /** Guards the once-per-attempt report (shared path vs. late joiner). */
    reported: boolean
}

let currentConfigLoad: ConfigLoadAttempt | undefined;
/** Bumped on every load (incl. force) so in-flight older responses cannot apply. */
let configLoadGeneration = 0;

/** Sentinel: a rejection value can legitimately be `undefined`, so it cannot mean "never reported". */
const NO_CONFIG_LOAD_ERROR_REPORTED = Symbol('none')
/**
 * Errors already surfaced by {@link reportConfigLoadFailure}.
 *
 * Object identity covers the normal case; {@link lastReportedConfigLoadError} covers
 * thrown strings/`null`, which have no identity a WeakSet could hold.
 */
const reportedConfigLoadErrors = new WeakSet<object>();
let lastReportedConfigLoadError: unknown = NO_CONFIG_LOAD_ERROR_REPORTED;

/**
 * Surface a failed config load — at most once per attempt, and only when an
 * interactive caller asked for it (`reportFailure`): the selected-instance watch,
 * the networkIsDisabled watch, `ensureConfigModeEditing`, forced reload and
 * discard all await the *same* shared attempt, so reporting per call site produced
 * 2–3 identical toasts. Poll-driven loads stay quiet — they are covered by the
 * stale indicator instead of a toast on every tick.
 */
const reportConfigLoadFailure = (attempt: ConfigLoadAttempt) => {
    if (attempt.reported) {
        return;
    }
    // Set `reported` only once this attempt actually reports: a poll-driven attempt
    // (reportFailure: false) must not consume the budget, or the interactive caller
    // joining later would be silenced by the guard it just tripped.
    if (!attempt.reportFailure || !managementAlive) {
        return;
    }
    attempt.reported = true;
    lastReportedConfigLoadError = attempt.rejection;
    if (attempt.rejection && typeof attempt.rejection === 'object') {
        reportedConfigLoadErrors.add(attempt.rejection);
    }
    toast.add({
        severity: 'error',
        summary: t('web.common.error'),
        detail: t('web.device_management.load_config_failed') + ': ' + errorDetail(attempt.rejection),
        life: TOAST_LIFE.error,
    });
};

/**
 * True when the same rejection was already reported by the shared config load.
 * Lets callers that wrap *several* loaders (e.g. `Promise.all`) keep their own
 * toast for the other loaders without repeating the config-load one.
 */
const wasConfigLoadFailureReported = (error: unknown): boolean =>
    error === lastReportedConfigLoadError
    || (!!error && typeof error === 'object' && reportedConfigLoadErrors.has(error));

const loadCurrentNetworkConfig = async (opts?: { force?: boolean, reportFailure?: boolean }) => {
    const selected = selectedInstanceId.value?.uuid;
    if (!selected) {
        clearNetworkConfigDraft();
        return;
    }

    if (!opts?.force && currentConfigLoad?.instanceId === selected) {
        // One shared attempt; report once if any joined caller wants the failure surfaced.
        const joined = currentConfigLoad;
        joined.reportFailure = joined.reportFailure || !!opts?.reportFailure;
        // A late joiner can arrive after the attempt rejected but before the entry is
        // cleared (that happens one microtask later), so its failure request would land
        // after the shared report already ran. Report here instead of staying silent.
        if (joined.settled) {
            reportConfigLoadFailure(joined);
        }
        return joined.promise;
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
        // Periodic clean reload: skip apply when body unchanged (avoid form flicker).
        if (!opts?.force && currentNetworkConfig.value?.instance_id === selected) {
            const nextSnap = captureNormalizedDirtySnapshot(ret).snapshot;
            if (nextSnap !== null && nextSnap === cleanConfigSnapshot.value) {
                return;
            }
        }
        applyLoadedNetworkConfig(ret);
    })();
    const attempt: ConfigLoadAttempt = {
        instanceId: selected,
        promise,
        reportFailure: !!opts?.reportFailure,
        settled: false,
        rejection: undefined,
        reported: false,
    };
    currentConfigLoad = attempt;
    // Interactive failures are reported here (once per attempt) instead of in each
    // caller's catch; `void` because the rejection is still propagated to callers.
    void promise.catch((e) => {
        attempt.settled = true;
        attempt.rejection = e;
        reportConfigLoadFailure(attempt);
    });
    try {
        await promise;
    } finally {
        if (currentConfigLoad === attempt) {
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
    // Start enables the existing DB row (keeps source). User→Web takeover is
    // only via Save / saveAndRun —those paths already confirm §8.1.5.
    void doStartNetwork();
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
        await loadCurrentNetworkConfig({ reportFailure: true });
        if (currentNetworkConfig.value?.instance_id === selected) {
            isEditingNetwork.value = true;
        }
    } catch (e: any) {
        console.error(e);
        if (!wasConfigLoadFailureReported(e)) {
            toast.add({
                severity: 'error',
                summary: t('web.common.error'),
                detail: t('web.device_management.load_config_failed') + ': ' + errorDetail(e),
                life: TOAST_LIFE.error,
            });
        }
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

/** P0-2: after delete RPC/DB failure, offer Retry while the console row remains. */
function offerRetryOnDeleteFailure(error: unknown, retryInstId: string): boolean {
    if (!managementAlive || !retryInstId) {
        return false;
    }
    const kind = Utils.classifyApiError(Utils.extractApiErrorPayload(error));
    if (!Utils.isRetryableDeleteApiError(kind)) {
        return false;
    }
    confirm.require({
        message: `${errorDetail(error)} ${t('web.device_management.delete_failed_retry_hint')}`,
        header: t('web.device_management.confirm_delete_retry_header'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('retry'),
            severity: 'danger',
        },
        // Capture id: user may switch the Select before accepting Retry.
        accept: () => { void doDeleteNetwork(retryInstId) },
    });
    return true;
}

const doDeleteNetwork = async (instId?: string) => {
    const deletingId = instId ?? instanceId.value;
    if (!deletingId) {
        return;
    }
    try {
        await props.api.delete_network(deletingId);
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
            life: TOAST_LIFE.severe,
        });
        // DB row kept on device-first delete failure —surface an explicit retry entry.
        offerRetryOnDeleteFailure(e, deletingId);
    }
    emits('update');
};

const confirmDeleteNetwork = () => {
    if (!canDeleteNetworkLocally.value) {
        return;
    }
    // §5: only the web-console surface warns when deleting a User-source row.
    // Do not infer this from deleteAuthority (explicit 'local' on console must still confirm).
    const deletingUserSourceOnConsole =
        props.hostSurface === 'web-console'
        && Api.normalizeConfigSource(currentNetworkMeta.value?.source) === Api.ConfigSource.User;
    confirm.require({
        message: t(
            deletingUserSourceOnConsole
                ? 'web.device_management.confirm_delete_user_source'
                : 'web.device_management.confirm_delete_network',
        ),
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
        accept: () => { void doDeleteNetwork() },
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
            const saveResult = await props.api.save_config(cfg);
            await props.api.update_network_instance_state(cfg.instance_id, false);
            // Surface prepare prediction after takeover Start, including
            // pending/unchanged, so offline/timeout never leaves the user
            // without feedback.
            const action = saveResult?.action;
            if (action) {
                toast.add({
                    severity: action === 'rebuilt' ? 'warn' : action === 'pending' ? 'info' : 'success',
                    summary: t('web.common.success'),
                    detail: saveResultToastDetail(saveResult),
                    life: TOAST_LIFE.success,
                });
            }
        } else {
            await props.api.run_network(cfg, currentNetworkControl.remoteSave.value);
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

    // 到这里启动/保存已经成功：下面的刷新只影响展示，失败不能再报成「启动网络失败」
    // （否则用户会重复点击 Run，导致实例被反复重建）。
    try {
        // Reload meta, then re-apply saved name so a still-running instance
        // (runtime identity lag) cannot leave the Select on the old label.
        delete networkMetaCache.value[cfg.instance_id];
        await loadNetworkMetas([cfg.instance_id]);
        patchMetaNetworkName(cfg.instance_id, cfg.network_name);

        setSelectedInstanceId(cfg.instance_id);
        await loadNetworkInstanceIds();
        await loadCurrentNetworkInfo();
    } catch (refreshError) {
        console.warn('post-start refresh failed (network is running)', refreshError);
        // 刷新失败也必须选中刚启动的网络，否则用户看不到它、界面上毫无提示。
        // 放在 catch 而非提前到 try 之前：提前赋值会让 selectedInstanceId watcher 在
        // loadNetworkMetas 的 await 期间 flush，而那时 meta 缓存刚被 delete，
        // 来源徽标 / takeover 判断这些 meta 驱动的 computed 会短暂读到空值。
        setSelectedInstanceId(cfg.instance_id);
    }

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
    // Footer @click must use confirmRunNetwork() —a bare handler would pass the MouseEvent.
    const cfg = isNetworkConfigArg(config) ? config : currentNetworkConfig.value;
    if (!cfg) {
        return;
    }
    // §8.1.5: User-source Run that Save→Web must confirm takeover (same as Save).
    if (shouldConfirmTakeover.value) {
        confirm.require({
            message: t('web.device_management.confirm_save_takeover_user_source'),
            header: t('run_network'),
            icon: 'pi pi-exclamation-triangle',
            rejectProps: {
                label: t('web.common.cancel'),
                severity: 'secondary',
                outlined: true,
            },
            acceptProps: {
                label: t('run_network'),
                severity: 'warning',
            },
            accept: () => { void saveAndRunNewNetwork(cfg) },
        });
        return;
    }
    // Re-run rebuilds a healthy instance (brief disconnect) —keep confirm.
    // First start of a stopped/disabled Web row used to warn about console→device
    // overwrite; that confirm was removed as redundant for day-to-day Start.
    const rerunning = !networkIsDisabled.value && selectedNetworkRunning.value;
    if (rerunning) {
        confirm.require({
            message: t('web.device_management.confirm_rerun_network'),
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
        return;
    }
    void saveAndRunNewNetwork(cfg);
}

const saveResultToastDetail = (
    result: Api.SaveNetworkConfigResponse | void,
    opts?: { wasRunning?: boolean },
) => {
    // GUI / local clients may return void —no console push semantics.
    if (!result || !result.action) {
        return props.hostSurface === 'gui-client' && opts?.wasRunning
            ? t('web.device_management.save_ok_local_hint')
            : t('web.device_management.save_ok_unchanged');
    }
    if (result.action === 'pending') {
        switch (result.reason) {
            case 'disabled':
                return t('web.device_management.save_ok_pending_disabled');
            case 'offline':
                return t('web.device_management.save_ok_pending_offline');
            case 'prepare_timeout':
                return t('web.device_management.save_ok_pending_prepare_timeout');
            default:
                return t('web.device_management.save_ok_pending');
        }
    }
    if (result.action === 'patched') {
        return t('web.device_management.save_ok_patched');
    }
    if (result.action === 'rebuilt') {
        return t('web.device_management.save_ok_rebuilt');
    }
    // GUI always reports `unchanged` (no hot-patch prediction). While running,
    // base fields are stored but not applied until the next Run.
    if (
        result.action === 'unchanged'
        && props.hostSurface === 'gui-client'
        && opts?.wasRunning
    ) {
        return t('web.device_management.save_ok_local_hint');
    }
    return t('web.device_management.save_ok_unchanged');
};

const isUserSourceConfig = computed(() => {
    const source = Api.normalizeConfigSource(currentNetworkMeta.value?.source);
    // Fail closed on missing meta (load failed / not loaded yet): without meta
    // we cannot prove Web ownership, so keep the takeover confirm. Unspecified
    // (old servers without source) keeps legacy behavior to avoid confirm spam.
    return source === undefined || source === Api.ConfigSource.User;
});

/**
 * Whether Save/Run must first confirm the User→Web takeover.
 *
 * Only the **web console** can actually take ownership: its `save_config` is a
 * REST upsert that unconditionally writes `source = web`. The desktop GUI saves
 * through the device (`save_network_config`) and never reports a User-source
 * config — the console would reject it as `OwnershipConflict` anyway — so the
 * confirmation there promises the opposite of what the same surface's own badge
 * tip states ("未接管前不会自动写入控制台数据库"). Unknown hosts keep the
 * confirm: we can only prove it is redundant for `gui-client`.
 */
const shouldConfirmTakeover = computed(() =>
    props.hostSurface !== 'gui-client' && isUserSourceConfig.value,
);

const doSaveNetworkConfig = async () => {
    if (!currentNetworkConfig.value || savingConfig.value || !canSaveConfig.value) {
        return;
    }
    savingConfig.value = true;
    const savedId = currentNetworkConfig.value.instance_id;
    const wasRunning = !!savedId && isRunning(savedId);
    try {
        const result = await props.api.save_config(currentNetworkConfig.value);
        const savedName = currentNetworkConfig.value.network_name;

        // 保存已经成功：下面的刷新只影响展示，失败不能再报成「保存失败」
        // （否则用户会以为没存上而重复保存）。
        try {
            delete networkMetaCache.value[savedId];
            await loadNetworkMetas([savedId]);
            patchMetaNetworkName(savedId, savedName);
            await loadNetworkInstanceIds();
        } catch (refreshError) {
            console.warn('post-save refresh failed (config was saved)', refreshError);
        }

        let alignedStart = false;
        if (shouldAutoStartAfterDisabledSave(result, wasRunning)) {
            try {
                await props.api.update_network_instance_state(savedId, false);
                await loadNetworkInstanceIds();
                await loadCurrentNetworkInfo();
                alignedStart = true;
            } catch (alignErr: any) {
                console.error(alignErr);
                toast.add({
                    severity: 'warn',
                    summary: t('web.common.warning'),
                    detail: t('web.device_management.save_ok_pending_disabled_still_running')
                        + ': ' + errorDetail(alignErr),
                    life: TOAST_LIFE.warn,
                });
                applyLoadedNetworkConfig(currentNetworkConfig.value);
                return;
            }
        }

        applyLoadedNetworkConfig(currentNetworkConfig.value);
        toast.add({
            severity: result?.action === 'rebuilt' ? 'warn' : 'success',
            summary: t('web.common.success'),
            detail: alignedStart
                ? t('web.device_management.save_ok_aligned_start')
                : saveResultToastDetail(result, { wasRunning }),
            life: TOAST_LIFE.success,
        });
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
};

/** User-source rows on the web console: confirm takeover before Save writes Web ownership. */
const saveNetworkConfig = async () => {
    if (!currentNetworkConfig.value || savingConfig.value || !canSaveConfig.value) {
        return;
    }
    if (shouldConfirmTakeover.value) {
        confirm.require({
            message: t('web.device_management.confirm_save_takeover_user_source'),
            header: t('web.device_management.save_config'),
            icon: 'pi pi-exclamation-triangle',
            rejectProps: {
                label: t('web.common.cancel'),
                severity: 'secondary',
                outlined: true,
            },
            acceptProps: {
                label: t('web.device_management.save_config'),
                severity: 'warning',
            },
            accept: () => { void doSaveNetworkConfig() },
        });
        return;
    }
    await doSaveNetworkConfig();
};
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

/** 「放弃修改」会丢弃草稿且不可恢复（只能靠重新加载找回来）——先确认。 */
const confirmDiscardConfigChanges = () => {
    if (!isConfigDirty.value) {
        return;
    }
    confirm.require({
        message: t('web.device_management.confirm_discard_changes'),
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
        accept: () => { void discardConfigChanges() },
    });
};

/** Discard unsaved draft by reloading last saved config. */
const discardConfigChanges = async () => {
    if (!isConfigDirty.value) {
        return;
    }
    try {
        await loadCurrentNetworkConfig({ force: true, reportFailure: true });
    } catch (e: any) {
        console.error(e);
        if (!wasConfigLoadFailureReported(e)) {
            toast.add({
                severity: 'error',
                summary: t('web.common.error'),
                detail: t('web.device_management.load_config_failed') + ': ' + errorDetail(e),
                life: TOAST_LIFE.error,
            });
        }
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
        // 这里是「进入编辑前读取配置」失败，不是保存失败（原复用 save_failed 文案误导）。
        toast.add({ severity: 'error', summary: t('web.common.error'), detail: t('web.device_management.load_config_failed') + ': ' + errorDetail(e), life: TOAST_LIFE.error });
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

    // S10: coalesce watch + PeriodicTask + post-start refresh onto one CollectNetworkInfo.
    const gen = networkInfoLoadGeneration;
    const network_info = await collectNetworkInfoSingleFlight(selected, () =>
        props.api.get_network_info(selected),
    );
    if (!managementAlive || gen !== networkInfoLoadGeneration) {
        return;
    }
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

/**
 * 轮询失败：置位内联「数据可能已过期」常驻提示，并按 30s 窗口最多补一条 warn toast；
 * 任一轮成功即清除。避免「面板继续显示旧数据但用户以为是最新」。
 */
const pollStale = ref(false)
const pollToastGate = createToastRepeatGate()

const pollOnce = async () => {
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
        // Config editor: probe console/device drift. Dirty →confirm prompt;
        // clean →get_config (GUI may adopt Web runtime into storage + emit).
        if (
            showConfigPanel.value
            && (pollTick === 1 || pollTick % CONFIG_MODE_LIST_POLL_EVERY === 0)
        ) {
            if (isConfigDirty.value) {
                jobs.push(checkServerConfigStaleWhileDirty())
            }
            else {
                jobs.push(loadCurrentNetworkConfig())
            }
        }
        if (jobs.length) {
            await Promise.all(jobs)
        }
        pollStale.value = false
    } catch (e) {
        console.debug(e);
        pollStale.value = true
        addThrottledToast(toast, pollToastGate, 'poll:remote-management', {
            severity: 'warn',
            summary: t('web.common.warning'),
            detail: t('web.device_management.data_stale'),
            life: TOAST_LIFE.warn,
        })
    }
}

/** 内联「数据可能已过期」条上的重试入口。 */
const retryPoll = () => { void pollOnce() }

let periodFunc = new Utils.PeriodicTask(pollOnce, STATUS_POLL_MS);

/** GUI host: console overwrite/hot-patch wrote localStorage —refresh open editor. */
const onManagedConfigSyncedOverwrite = (event: Event) => {
    const detail = (event as CustomEvent<{ instanceId?: string }>).detail;
    const syncedId = detail?.instanceId;
    if (!syncedId || selectedInstanceId.value?.uuid !== syncedId) {
        return;
    }
    // Invalidate meta so ownership badge refreshes.
    delete networkMetaCache.value[syncedId];
    void loadNetworkMetas([syncedId]);
    if (!(showConfigPanel.value || isCombinedMode.value)) {
        return;
    }
    // Dirty draft: ask before discarding local edits (console is authoritative
    // once the user accepts reload).
    if (isConfigDirty.value) {
        offerReloadOnServerConfigUpdated();
        return;
    }
    void loadCurrentNetworkConfig({ force: true }).catch((e) => {
        console.debug('reload after managed sync failed', e);
    });
};

onMounted(async () => {
    managementAlive = true;
    periodFunc.start();
    window.addEventListener('managed_config_synced_overwrite', onManagedConfigSyncedOverwrite);
});

onUnmounted(() => {
    managementAlive = false;
    window.removeEventListener('managed_config_synced_overwrite', onManagedConfigSyncedOverwrite);
    ++configLoadGeneration; // invalidate in-flight load/probe apply
    ++networkInfoLoadGeneration; // discard late CollectNetworkInfo apply (S10)
    periodFunc.stop();
    networkMetaCache.value = {};
    networkMetaLoadFailed.value = {};
});

</script>


<template>
    <div class="device-management" :class="{ 'device-management--page': fullPage || !!drawerClose }">
        <input type="file" @change="handleFileUpload" class="hidden" accept="application/toml" ref="configFile" />

        <!-- 网络选择和操作按钮始终在同一行（顶栏固定） -->
        <!-- 轮询失败：常驻提示（toast 会消失，这里不会），成功自动清除 -->
        <div v-if="pollStale" class="et-meta flex flex-row items-center gap-2 py-1 network-stale" role="status">
            <i class="pi pi-exclamation-triangle" aria-hidden="true"></i>
            <span>{{ t('web.device_management.data_stale') }}</span>
            <Button
                :label="t('web.common.retry')"
                icon="pi pi-refresh"
                severity="secondary"
                size="small"
                text
                @click="retryPoll"
            />
        </div>

        <div class="network-header">
            <h1 v-if="pageTitle" class="et-page-title network-page-title">{{ pageTitle }}</h1>
            <div class="network-header-main flex flex-row justify-between items-center gap-2">
                <!-- 网络选择下拉 -->
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
                                        <span v-if="slotProps.value.meta || currentNetworkConfig?.instance_id === slotProps.value.uuid">
                                            {{ displayNetworkName(slotProps.value.uuid, slotProps.value.meta) }} ({{ slotProps.value.uuid }})
                                        </span>
                                        <span v-else>{{ slotProps.value.uuid }}</span>
                                    </span>
                                    <Tag
                                        v-if="currentConfigSourceBadge"
                                        class="network-source-tag network-source-tag--icon-only leading-3 shrink-0"
                                        :severity="currentConfigSourceBadge.severity"
                                        :icon="currentConfigSourceBadge.icon"
                                        :aria-label="currentConfigSourceBadge.label"
                                        v-tooltip.top="currentConfigSourceBadge.tip"
                                    />
                                    <Tag v-if="isConfigDirty && showConfigPanel" class="network-dirty-tag leading-3 shrink-0"
                                        severity="warn"
                                        :value="t('web.device_management.unsaved_changes')" />
                                    <Tag v-if="residualKindFor(slotProps.value.uuid)" class="network-residual-tag leading-3 shrink-0"
                                        severity="warn"
                                        :value="residualBadgeLabel(residualKindFor(slotProps.value.uuid)!)" />
                                    <Tag class="network-status-tag leading-3 shrink-0"
                                        :severity="isRunning(slotProps.value.uuid) ? 'success' : 'danger'"
                                        :value="t(isRunning(slotProps.value.uuid) ? 'network_running' : 'network_stopped')" />
                                    <!-- Roadmap §2.3「最上方（实例态旁）」：VIP 可达 [M/N]，与实例运行态并排 -->
                                    <Tag v-if="vipBadgeFor(slotProps.value.uuid)?.text"
                                        class="vip-reachability-tag leading-3 shrink-0"
                                        :severity="vipBadgeFor(slotProps.value.uuid)?.severity"
                                        :value="vipBadgeFor(slotProps.value.uuid)?.text"
                                        v-tooltip.top="vipBadgeFor(slotProps.value.uuid)?.tip" />
                                </div>
                                <span v-else class="network-select-placeholder">{{ slotProps.placeholder }}</span>
                            </template>
                            <template #option="slotProps">
                                <div class="flex flex-col items-start content-center max-w-full">
                                    <div class="flex items-center min-w-0 gap-2 w-full">
                                        <div class="min-w-0 flex-1">
                                            <span class="truncate block">{{ t('network_name') }}: {{
                                                displayNetworkName(slotProps.option.uuid, slotProps.option.meta) }}</span>
                                        </div>
                                        <Tag
                                            v-if="configSourceBadgeById[slotProps.option.uuid]"
                                            class="network-source-tag network-source-tag--icon-only leading-3 shrink-0"
                                            :severity="configSourceBadgeById[slotProps.option.uuid].severity"
                                            :icon="configSourceBadgeById[slotProps.option.uuid].icon"
                                            :aria-label="configSourceBadgeById[slotProps.option.uuid].label"
                                            v-tooltip.top="configSourceBadgeById[slotProps.option.uuid].tip"
                                        />
                                        <Tag v-if="residualKindFor(slotProps.option.uuid)" class="network-residual-tag leading-3 shrink-0"
                                            severity="warn"
                                            :value="residualBadgeLabel(residualKindFor(slotProps.option.uuid)!)" />
                                        <Tag class="network-status-tag leading-3 shrink-0"
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

        <Message v-if="currentResidualKind" severity="warn" class="mb-0 residual-banner shrink-0" :closable="false">
            <div class="flex flex-wrap items-center gap-2">
                <span class="flex-1 min-w-0">
                    {{ currentResidualKind === 'device_only'
                        ? t('web.device_management.residual_device_only_hint')
                        : t('web.device_management.residual_db_disabled_running_hint') }}
                </span>
                <Button
                    class="residual-action-btn"
                    size="small"
                    severity="danger"
                    outlined
                    :label="t('web.device_management.clear_device_copy')"
                    @click="clearResidualDeviceCopy"
                    v-tooltip.bottom="t('web.device_management.clear_device_copy_tip')"
                />
                <Button
                    class="residual-action-btn"
                    size="small"
                    severity="success"
                    outlined
                    :label="t('web.device_management.takeover_to_console')"
                    @click="takeoverResidualToConsole"
                    v-tooltip.bottom="t('web.device_management.takeover_to_console_tip')"
                />
                <Button
                    class="residual-action-btn"
                    size="small"
                    severity="secondary"
                    text
                    :label="t('web.device_management.ignore_residual')"
                    @click="ignoreCurrentResidual"
                    v-tooltip.bottom="t('web.device_management.ignore_residual_tip')"
                />
            </div>
        </Message>

        <!-- 仅中间表单/状态区滚动 -->
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
                                    <!-- Tooltip on wrapper: native disabled buttons do not fire mouseenter. -->
                                    <span
                                        v-if="selectedInstanceId && currentNetworkControl.deletable.value"
                                        v-tooltip.bottom="deleteNetworkTooltip"
                                        class="inline-flex">
                                        <Button class="config-toolbar-btn" @click="confirmDeleteNetwork" icon="pi pi-trash"
                                            :label="t('web.device_management.delete_network')" iconPos="left" severity="danger"
                                            outlined
                                            :disabled="isConsoleManagedDelete" />
                                    </span>
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
                    class="network-footer-btn network-footer-btn--muted" @click="confirmDiscardConfigChanges"
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
    color: var(--text-color, #0f172a);
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
    font-size: var(--et-fs-body, 0.9375rem);
    color: var(--text-color-secondary, #334155);
}

.network-select-placeholder {
    color: var(--et-placeholder-color, #94a3b8);
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
    font-size: var(--et-fs-body, 0.9375rem) !important;
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
    color: var(--text-color, #0f172a) !important;
}

:deep(.network-footer-btn--muted.p-button:hover:not(:disabled)),
:deep(.network-footer-btn.p-button.p-button-secondary:hover:not(:disabled)) {
    background: var(--surface-200, #e2e8f0) !important;
    border-color: color-mix(in srgb, var(--primary-color, #0ea5e9) 35%, var(--et-border-color, #e2e8f0)) !important;
    color: var(--text-color, #0f172a) !important;
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
    font-size: var(--et-fs-body, 0.9375rem) !important;
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
    font-size: var(--et-fs-body, 0.9375rem) !important;
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

/* 实例状态配色：运行中绿色，已停止红色 */
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

/* Trailing tags: shared weight/padding; soft chips in the overlay / default surface. */
:deep(.network-status-tag.p-tag),
:deep(.network-dirty-tag.p-tag),
:deep(.network-residual-tag.p-tag),
:deep(.network-source-tag.p-tag) {
    font-weight: 600 !important;
    letter-spacing: 0.02em;
    padding: 0.2rem 0.5rem !important;
    border: none !important;
    gap: 0.25rem;
}

:deep(.network-dirty-tag.p-tag),
:deep(.network-residual-tag.p-tag),
:deep(.network-source-tag.p-tag-warn) {
    background: color-mix(in srgb, #f59e0b 18%, #ffffff) !important;
    color: #b45309 !important;
}

/* Ownership: icon only; label stays in aria-label + tooltip. */
:deep(.network-source-tag--icon-only.p-tag) {
    padding: 0.25rem !important;
    gap: 0;
    background: transparent !important;
    background-color: transparent !important;
}

:deep(.network-source-tag--icon-only.p-tag .p-tag-label) {
    display: none;
}

:deep(.network-source-tag.p-tag-info) {
    background: color-mix(in srgb, var(--primary-color, #0ea5e9) 16%, #ffffff) !important;
    color: var(--primary-color, #0284c7) !important;
}

:deep(.network-source-tag.p-tag-secondary) {
    background: color-mix(in srgb, var(--text-color-secondary, #334155) 12%, #ffffff) !important;
    color: #475569 !important;
}

/* Icon-only ownership: keep severity tint on the glyph, no soft chip fill. */
:deep(.network-source-tag--icon-only.p-tag-info),
:deep(.network-source-tag--icon-only.p-tag-secondary),
:deep(.network-source-tag--icon-only.p-tag-warn) {
    background: transparent !important;
    background-color: transparent !important;
}

/* Run status text: tinted label only, no soft chip fill (align with icon-only source). */
:deep(.network-status-tag.p-tag-success),
:deep(.p-select-overlay .network-status-tag.p-tag-success),
:deep(.network-select-overlay .network-status-tag.p-tag-success) {
    background: transparent !important;
    background-color: transparent !important;
    color: #047857 !important;
}

:deep(.network-status-tag.p-tag-danger),
:deep(.p-select-overlay .network-status-tag.p-tag-danger),
:deep(.network-select-overlay .network-status-tag.p-tag-danger) {
    background: transparent !important;
    background-color: transparent !important;
    color: #b91c1c !important;
}

/* On the filled running/stopped select: all trailing tags share the same chrome. */
:deep(.network-instance-select--running .network-source-tag.p-tag),
:deep(.network-instance-select--stopped .network-source-tag.p-tag),
:deep(.network-instance-select--running .network-status-tag.p-tag),
:deep(.network-instance-select--stopped .network-status-tag.p-tag),
:deep(.network-instance-select--running .network-dirty-tag.p-tag),
:deep(.network-instance-select--stopped .network-dirty-tag.p-tag),
:deep(.network-instance-select--running .network-residual-tag.p-tag),
:deep(.network-instance-select--stopped .network-residual-tag.p-tag) {
    background: transparent !important;
    background-color: transparent !important;
    color: #ffffff !important;
    border: none !important;
    padding: 0.15rem !important;
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
    font-size: var(--et-fs-meta, 0.8125rem);
    font-weight: 600;
    color: var(--text-color-secondary, #334155);
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
 * 移动端（≤640px）：一行一个按钮太浪费纵向空间，改成「每组一行铺满」的网格。
 *
 * 一行能放几个，由最长文案 `编辑为文件`（5 个汉字）决定：
 *   手机内容区宽 ≈ 屏宽 − 45px（et-main-content 0.75rem×2 + 卡片 0.6rem×2 + 边框）
 *   360px 屏 → 约 315px 可用
 *   · 带图标：5×14(字) + 16(图标) + 8(间距) + 16(内边距) ≈ 110px → 315/110 = 2.9，一行最多 2~3 个
 *   · 去图标 + 13px 字：5×13 + 8(内边距) ≈ 73px → 315/73 = 4.3，一行放得下 4 个
 * 所以移动端去掉图标、字号降到 0.8125rem，换取一行 4 个。
 *
 * 分组后的行数：配置文件组 3~4 个 → 一行放完；网络组 1~2 个 → 一行放完；底栏 3 个 → 一行 3 个。
 * 用 auto-fit + minmax(0, 1fr) 让每组把整行等分，宽度不受文案长短影响，也不会出现 3+1 的残缺行。
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
        /* 窄屏：底部按钮紧凑排列 */
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

/* 按钮样式 */
.button-container {
    gap: 0.5rem;
    align-items: center;
}

/* 菜单样式定制 */
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

/* 仅顶栏纯图标按钮，避免覆盖底栏带文字按钮 */
:deep(.header-action-btn.p-button-icon-only .p-button-icon) {
    font-size: 1rem;
    margin: 0;
}

/* 网络选择相关样式 */
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
    color: var(--text-color-secondary, #334155);
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
 * 平板 / 窄窗口（641–768px）：按钮统一收窄到 8.5rem，刚好容纳最长文案「返回设备列表」，
 * 一行能放下 4 个工具栏按钮。
 * 必须限定 min-width: 641px —— 否则它会在 ≤640px 也命中，把下面网格布局的 min-width: 0 盖掉。
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
