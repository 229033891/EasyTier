<script setup lang="ts">
import { Button, ConfirmDialog, ConfirmPopup, Menu, Message, Select, Tag, useConfirm, useToast, type VirtualScrollerLazyEvent } from 'primevue';
import { computed, onMounted, onUnmounted, Ref, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import * as Api from '../modules/api';
import * as Utils from '../modules/utils';
import * as NetworkTypes from '../types/network';
import { type MenuItem } from 'primevue/menuitem';

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
    const responseData = (error as { response?: { data?: unknown } } | null)?.response?.data;
    if (typeof responseData === 'string') {
        return responseData;
    }
    if (responseData !== undefined) {
        try {
            return JSON.stringify(responseData);
        } catch {
            return String(responseData);
        }
    }
    return error instanceof Error ? error.message : String(error);
}

const configFile = ref();

const curNetworkInfo = ref<NetworkTypes.NetworkInstance | null>(null);

const showConfigEditDialog = ref(false);
const isEditingNetwork = ref(false); // Flag to indicate if we're in network editing mode
const currentNetworkConfig = ref<NetworkTypes.NetworkConfig | undefined>(undefined);

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

/** 配置页常显保存：有配置且可编辑（运行中也可只存盘不重跑） */
const canSaveConfig = computed(() => {
    if (!currentNetworkConfig.value) {
        return false;
    }
    // 元数据尚未加载或加载失败时拒绝操作，避免把未知权限当成可写。
    if (!currentNetworkMeta.value || networkMetaLoadFailed.value[currentNetworkConfig.value.instance_id]) {
        return false;
    }
    return currentNetworkControl.editable.value;
});

const savingConfig = ref(false);

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
    set(value: any) {
        instanceId.value = value ? value.uuid : undefined;
    }
});
watch(selectedInstanceId, async (newVal, oldVal) => {
    try {
        if (newVal?.uuid !== oldVal?.uuid && (networkIsDisabled.value || isEditingNetwork.value)) {
            await loadCurrentNetworkConfig();
        } else {
            await loadCurrentNetworkInfo();
        }

        if (newVal?.uuid && !networkMetaCache.value[newVal.uuid]) {
            await loadNetworkMetas([newVal.uuid]);
        }
    } catch (e) {
        console.error('Failed to load selected network', e);
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: errorDetail(e),
            life: 3000,
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

/** 配置编辑面板：配置模式 / 合一模式的编辑与禁用态 */
const showConfigPanel = computed(() => {
    if (isStatusMode.value) {
        return false;
    }
    if (isEditingNetwork.value || networkIsDisabled.value) {
        return true;
    }
    // 配置模式已选中实例时由表单占位（ensureConfigModeEditing 会拉配置）
    return isConfigMode.value && !!selectedInstanceId.value;
})

/** 状态模式：已禁用虚拟网时显示启停，不进配置表单 */
const showStatusDisabledPanel = computed(() => {
    return isStatusMode.value && !!selectedInstanceId.value && networkIsDisabled.value;
})

const networkIsDisabled = computed(() => {
    if (!selectedInstanceId.value) {
        return false;
    }
    return disabledInstanceIds.value.has(selectedInstanceId.value.uuid);
});
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
                life: 3000,
            });
        }
    }
});

let currentConfigLoad: { instanceId: string; promise: Promise<void> } | undefined;

const loadCurrentNetworkConfig = async () => {
    const selected = selectedInstanceId.value?.uuid;
    if (!selected) {
        currentNetworkConfig.value = undefined;
        return;
    }

    if (currentConfigLoad?.instanceId === selected) {
        return currentConfigLoad.promise;
    }

    currentNetworkConfig.value = undefined;
    const promise = (async () => {
        const ret = await props.api.get_network_config(selected);
        if (selectedInstanceId.value?.uuid === selected) {
            currentNetworkConfig.value = ret;
        }
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

    await props.api.update_network_instance_state(selectedInstanceId.value.uuid, true);
    await loadNetworkInstanceIds();
}

const confirmStopNetwork = (event: Event) => {
    confirm.require({
        target: event.currentTarget as HTMLElement,
        message: t('web.device_management.confirm_disable_network'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.device_management.disable_network'),
            severity: 'danger',
        },
        accept: () => { void stopNetwork() },
    });
}

/** 状态模式：启用已禁用的虚拟网 */
const startNetwork = async () => {
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
            life: 3000,
        });
    }
}

const confirmStartNetwork = (event: Event) => {
    confirm.require({
        target: event.currentTarget as HTMLElement,
        message: t('web.device_management.confirm_start_network'),
        icon: 'pi pi-info-circle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.network.start'),
            severity: 'success',
        },
        accept: () => { void startNetwork() },
    });
}

const requestSwitchMode = (mode: 'status' | 'config') => {
    emits('switchMode', mode);
}

/** 配置模式：选中运行中实例时自动进入编辑表单 */
const ensureConfigModeEditing = async () => {
    if (!isConfigMode.value || !selectedInstanceId.value || networkIsDisabled.value) {
        return;
    }
    if (isEditingNetwork.value && currentNetworkConfig.value?.instance_id === selectedInstanceId.value.uuid) {
        return;
    }
    try {
        await loadCurrentNetworkConfig();
        if (currentNetworkConfig.value?.instance_id === selectedInstanceId.value.uuid) {
            isEditingNetwork.value = true;
        }
    } catch (e) {
        console.error(e);
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
                    life: 2000,
                });
            } catch (e) {
                console.error(e);
                toast.add({
                    severity: 'error',
                    summary: t('web.device_management.delete_network'),
                    detail: String(e),
                    life: 3000,
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

        selectedInstanceId.value = { uuid: cfg.instance_id };
        await loadNetworkInstanceIds();
        await loadCurrentNetworkInfo();
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: t('web.common.error'), detail: t('web.device_management.start_failed') + ': ' + errorDetail(e), life: 3000 });
        return;
    }

    emits('update');
    if (isConfigMode.value) {
        // 配置模式保存/运行后仍留在配置表单
        isEditingNetwork.value = true;
    } else {
        isEditingNetwork.value = false;
    }
}

const saveNetworkConfig = async () => {
    if (!currentNetworkConfig.value || savingConfig.value) {
        return;
    }
    savingConfig.value = true;
    try {
        await props.api.save_config(currentNetworkConfig.value);

        delete networkMetaCache.value[currentNetworkConfig.value.instance_id];
        await loadNetworkMetas([currentNetworkConfig.value.instance_id]);

        toast.add({
            severity: 'success',
            summary: t('web.common.success'),
            detail: t('web.device_management.config_saved'),
            life: 2000,
        });
    } catch (e: any) {
        console.error(e);
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: t('web.device_management.save_failed') + ': ' + errorDetail(e),
            life: 3000,
        });
    } finally {
        savingConfig.value = false;
    }
}
const newNetwork = async () => {
    const newNetworkConfig = props.newConfigGenerator?.() ?? NetworkTypes.DEFAULT_NETWORK_CONFIG();
    try {
        await props.api.save_config(newNetworkConfig);
        selectedInstanceId.value = { uuid: newNetworkConfig.instance_id };
        currentNetworkConfig.value = newNetworkConfig;
        isEditingNetwork.value = true;
        await loadNetworkInstanceIds();
    } catch (e) {
        console.error(e);
        toast.add({
            severity: 'error',
            summary: t('web.common.error'),
            detail: t('web.device_management.save_failed') + ': ' + errorDetail(e),
            life: 3000,
        });
    }
}

const cancelEditNetwork = () => {
    isEditingNetwork.value = false;
}

const editNetwork = async () => {
    if (!instanceId.value) {
        toast.add({ severity: 'error', summary: 'Error', detail: 'No network instance selected', life: 2000 });
        return;
    }

    try {
        const ret = await props.api.get_network_config(instanceId.value!);
        currentNetworkConfig.value = ret;
        isEditingNetwork.value = true; // Switch to editing mode instead
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: t('web.common.error'), detail: t('web.device_management.save_failed') + ': ' + errorDetail(e), life: 3000 });
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
        toast.add({ severity: 'error', summary: 'Error', detail: 'No network instance selected', life: 2000 });
        return;
    }

    const targetInstanceId = instanceId.value;
    try {
        const { instance_id, ...networkConfig } = await props.api.get_network_config(targetInstanceId);
        let { toml_config: tomlConfig, error } = await props.api.generate_config(networkConfig as NetworkTypes.NetworkConfig);
        if (error) {
            throw { response: { data: error } };
        }
        if (instanceId.value !== targetInstanceId) {
            return;
        }
        exportTomlFile(tomlConfig ?? '', targetInstanceId + '.toml');
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: t('web.common.error'), detail: t('web.device_management.export_config') + ': ' + errorDetail(e), life: 3000 });
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
                throw resp.error;
            }

            const config = resp.config;
            if (!config) return;
            if (targetInstanceId && instanceId.value !== targetInstanceId) {
                return;
            }

            config.instance_id = targetInstanceId ?? config.instance_id;
            currentNetworkConfig.value = config;
            toast.add({ severity: 'success', summary: 'Import Success', detail: "Config file import success", life: 2000 });
        } catch (error) {
            toast.add({ severity: 'error', summary: 'Error', detail: 'Config file parse error: ' + error, life: 2000 });
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

// 响应式屏幕宽度
const screenWidth = ref(window.innerWidth);
const updateScreenWidth = () => {
    screenWidth.value = window.innerWidth;
};

// 菜单：仅保留「编辑」等不适合常显的操作；导出/删除已平铺到配置工具栏
const menuRef = ref();
const actionMenu: Ref<MenuItem[]> = ref([
    {
        label: () => t('web.device_management.edit_network'),
        icon: 'pi pi-pencil',
        // 状态模式不提供编辑；配置模式已在表单中则隐藏
        visible: () =>
            !isStatusMode.value
            && !isEditingNetwork.value
            && !(networkIsDisabled.value ?? true)
            && currentNetworkControl.editable.value,
        command: () => editNetwork()
    },
]);

const showMoreActionsMenu = computed(() =>
    !isStatusMode.value
    && !!selectedInstanceId.value
    && (isCombinedMode.value ? !isEditingNetwork.value : false)
    && actionMenu.value.some((item) => item.visible === undefined || (typeof item.visible === 'function' ? item.visible() : item.visible))
);

const showHeaderActions = computed(() =>
    (isCombinedMode.value && isEditingNetwork.value) || showMoreActionsMenu.value
);

/** 底部主操作：按当前面板决定右侧按钮 */
const stickyFooterPrimary = computed(() => {
    if (showStatusDisabledPanel.value) {
        return 'start' as const;
    }
    if (showConfigPanel.value) {
        return 'run' as const;
    }
    if (needShowNetworkStatus.value) {
        return 'stop' as const;
    }
    return 'none' as const;
});

const showStickyFooter = computed(() =>
    showLeaveInFooter.value || stickyFooterPrimary.value !== 'none'
);

let periodFunc = new Utils.PeriodicTask(async () => {
    if (props.pauseAutoRefresh) {
        return;
    }
    try {
        await Promise.all([loadNetworkInstanceIds(), loadCurrentNetworkInfo()]);
    } catch (e) {
        console.debug(e);
    }
}, 1000);

onMounted(async () => {
    periodFunc.start();

    // 添加屏幕尺寸监听
    window.addEventListener('resize', updateScreenWidth);
});

onUnmounted(() => {
    periodFunc.stop();

    // 移除屏幕尺寸监听
    window.removeEventListener('resize', updateScreenWidth);
});

</script>


<template>
    <div class="device-management" :class="{ 'device-management--page': fullPage || !!drawerClose }">
        <input type="file" @change="handleFileUpload" class="hidden" accept="application/toml" ref="configFile" />

        <!-- 网络选择和操作按钮始终在同一行（顶栏固定） -->
        <div class="network-header">
            <h1 v-if="pageTitle" class="et-page-title network-page-title">{{ pageTitle }}</h1>
            <div class="flex flex-row justify-between items-center gap-2">
                <!-- 网络选择 -->
                <div class="flex-1 min-w-0">
                    <Select v-model="selectedInstanceId" :options="instanceList" optionLabel="uuid" class="w-full"
                        inputId="dd-inst-id" :placeholder="t('web.device_management.select_network')"
                        :pt="{ root: { class: 'network-select-container' } }" :virtualScrollerOptions="{
                            lazy: true,
                            onLazyLoad: onLazyLoadNetworkMetas,
                            itemSize: 60,
                            delay: 50
                        }">
                            <template #value="slotProps">
                                <div v-if="slotProps.value" class="flex items-center min-w-0 gap-2">
                                    <span class="truncate block min-w-0 flex-1">
                                        <span v-if="slotProps.value.meta">
                                            {{ slotProps.value.meta.network_name }} ({{ slotProps.value.uuid }})
                                        </span>
                                        <span v-else>{{ slotProps.value.uuid }}</span>
                                    </span>
                                    <Tag class="leading-3 shrink-0"
                                        :severity="isRunning(slotProps.value.uuid) ? 'success' : 'info'"
                                        :value="t(isRunning(slotProps.value.uuid) ? 'network_running' : 'network_stopped')" />
                                </div>
                                <span v-else>{{ slotProps.placeholder }}</span>
                            </template>
                            <template #option="slotProps">
                                <div class="flex flex-col items-start content-center max-w-full">
                                    <div class="flex items-center min-w-0">
                                        <div class="mr-4 min-w-0 flex-1">
                                            <span class="truncate block">{{ t('network_name') }}: {{
                                                slotProps.option.meta?.network_name ?? slotProps.option.uuid }}</span>
                                        </div>
                                        <Tag class="my-auto leading-3 shrink-0"
                                            :severity="isRunning(slotProps.option.uuid) ? 'success' : 'info'"
                                            :value="t(isRunning(slotProps.option.uuid) ? 'network_running' : 'network_stopped')" />
                                    </div>
                                    <div class="max-w-full overflow-hidden text-ellipsis text-gray-500">
                                        {{ slotProps.option.uuid }}
                                    </div>
                                </div>
                            </template>
                        </Select>
                </div>

                <!-- 顶栏：取消编辑 / 更多（创建网络改到配置工具栏） -->
                <div v-if="showHeaderActions" class="flex gap-2 shrink-0 button-container items-center">
                    <Button v-if="isCombinedMode && isEditingNetwork" @click="cancelEditNetwork" icon="pi pi-times"
                        :label="screenWidth > 640 ? t('web.device_management.cancel_edit') : undefined"
                        :class="['header-action-btn', screenWidth <= 640 ? 'p-button-icon-only' : '']"
                        v-tooltip.bottom="screenWidth <= 640 ? t('web.device_management.cancel_edit') : undefined"
                        severity="secondary" />

                    <!-- More actions menu（仅 GUI 合一模式的「编辑」入口） -->
                    <Menu ref="menuRef" :model="actionMenu" :popup="true" />
                    <Button v-if="showMoreActionsMenu"
                        icon="pi pi-ellipsis-v"
                        class="header-action-btn header-action-btn--icon" severity="secondary"
                        @click="menuRef.toggle($event)" :aria-label="t('web.device_management.more_actions')"
                        v-tooltip.bottom="t('web.device_management.more_actions')" />
                </div>
            </div>
        </div>

        <!-- 配置工具栏固定在顶栏下方，不随表单滚动 -->
        <div v-if="showConfigPanel" class="network-toolbar">
            <div class="config-toolbar">
                <div class="toolbar-zone">
                    <span class="toolbar-zone-label">{{ t('web.device_management.toolbar_config_files') }}</span>
                    <div class="toolbar-zone-actions">
                        <Button class="config-toolbar-btn" @click="showConfigEditDialog = true" icon="pi pi-file-edit"
                            :label="t('web.device_management.edit_as_file')" iconPos="left" severity="secondary"
                            v-tooltip.bottom="t('web.device_management.edit_as_file_tip')" />
                        <Button class="config-toolbar-btn" @click="importConfig" icon="pi pi-upload"
                            :label="t('web.device_management.import_config')" iconPos="left" severity="secondary"
                            v-tooltip.bottom="t('web.device_management.import_config_tip')" />
                        <Button v-if="selectedInstanceId" class="config-toolbar-btn" @click="exportConfig" icon="pi pi-download"
                            :label="t('web.device_management.export_config')" iconPos="left" severity="secondary"
                            v-tooltip.bottom="t('web.device_management.export_config_tip')" />
                        <Button v-if="canSaveConfig" class="config-toolbar-btn" @click="saveNetworkConfig"
                            :disabled="!currentNetworkConfig || savingConfig"
                            icon="pi pi-save" :label="t('web.device_management.save_config')" iconPos="left"
                            severity="success"
                            v-tooltip.bottom="t('web.device_management.save_config_tip')" />
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
        </div>

        <!-- 仅中间表单/状态区滚动 -->
        <div class="network-content">
            <Message v-if="showStatusDisabledPanel" severity="warn" class="mb-0">
                {{ t('web.device_management.network_disabled_hint') }}
            </Message>

            <template v-else-if="showConfigPanel && currentNetworkConfig">
                <Config :cur-network="currentNetworkConfig" :config-invalid="false"
                    :hide-run-button="true" @run-network="saveAndRunNewNetwork"></Config>
            </template>
            <Message v-else-if="showConfigPanel" severity="info" class="mb-0">
                {{ t('web.device_management.loading_network_configuration') }}
            </Message>

            <template v-else-if="needShowNetworkStatus">
                <Status v-if="curNetworkInfo && curNetworkInfo.error_msg === ''" v-bind:cur-network-inst="curNetworkInfo"
                    :api="api" class="mb-0" />
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
            <Button v-if="showLeaveInFooter" @click="drawerClose" :label="leaveLabel" severity="secondary"
                :icon="leaveIcon" iconPos="left" class="network-footer-btn"
                v-tooltip.top="leaveTooltip" />
            <div class="footer-zone footer-zone--primary">
                <Button v-if="isConfigMode" icon="pi pi-chart-line" severity="secondary"
                    :label="t('web.device_management.switch_to_status')" iconPos="left"
                    class="network-footer-btn" @click="requestSwitchMode('status')"
                    v-tooltip.top="t('web.device_management.switch_to_status_tip')" />
                <Button v-else-if="isStatusMode" icon="pi pi-cog" severity="secondary"
                    :label="t('web.device_management.switch_to_config')" iconPos="left"
                    class="network-footer-btn" @click="requestSwitchMode('config')"
                    v-tooltip.top="t('web.device_management.switch_to_config_tip')" />

                <Button v-if="stickyFooterPrimary === 'start'" @click="confirmStartNetwork($event)"
                    :disabled="!currentNetworkControl.deletable.value" :label="t('web.network.start')"
                    severity="success" icon="pi pi-play" iconPos="left" class="network-footer-btn"
                    v-tooltip.top="t('web.device_management.start_network_tip')" />
                <Button v-else-if="stickyFooterPrimary === 'run'"
                    @click="saveAndRunNewNetwork(currentNetworkConfig!)" :disabled="!currentNetworkConfig"
                    :label="t('run_network')" severity="success" icon="pi pi-arrow-right" iconPos="right"
                    class="network-footer-btn"
                    v-tooltip.top="t('run_network_tip')" />
                <Button v-else-if="stickyFooterPrimary === 'stop'" @click="confirmStopNetwork($event)"
                    :disabled="!currentNetworkControl.deletable.value"
                    :label="t('web.device_management.disable_network')" severity="danger" icon="pi pi-power-off"
                    iconPos="left" class="network-footer-btn"
                    v-tooltip.top="t('web.device_management.disable_network_tip')" />
            </div>
        </div>

        <ConfirmPopup />
        <ConfirmDialog />

        <ConfigEditDialog v-model:visible="showConfigEditDialog" :cur-network="currentNetworkConfig"
            :generate-config="generateConfig" :save-config="syncTomlConfig" />
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
    border: var(--et-border, 1px solid var(--surface-border, #e5e7eb));
    border-radius: var(--et-radius, 0.5rem);
    box-shadow: var(--et-shadow-card, 0 1px 2px rgba(15, 23, 42, 0.03));
    padding: 0.5rem 0.75rem 0.6rem !important;
    margin: 0 !important;
}

.network-page-title {
    margin: 0 0 0.1rem;
    font-size: var(--et-fs-page-title, 1.25rem);
    font-weight: 700;
    line-height: 1.25;
}

.network-toolbar {
    flex-shrink: 0;
    z-index: 20;
    background: var(--surface-card, #ffffff) !important;
    border: var(--et-border, 1px solid var(--surface-border, #e5e7eb));
    border-radius: var(--et-radius, 0.5rem);
    padding: 0.35rem 0.6rem 0.4rem;
    margin: 0;
}

.network-content {
    flex: 1 1 auto;
    overflow-y: auto;
    min-height: 0;
    padding: 0.4rem 0.6rem !important;
    background: var(--surface-card, #ffffff) !important;
    border: var(--et-border, 1px solid var(--surface-border, #e5e7eb));
    border-radius: var(--et-radius, 0.5rem);
    box-shadow: var(--et-shadow-card, 0 1px 2px rgba(15, 23, 42, 0.03)) !important;
}

.network-sticky-footer {
    flex-shrink: 0;
    z-index: 20;
    margin-top: 0;
    padding: 0.4rem 0.6rem;
    background: var(--surface-card, #ffffff);
    border: var(--et-border, 1px solid var(--surface-border, #e5e7eb));
    border-radius: var(--et-radius, 0.5rem);
    box-shadow: var(--et-shadow-card, 0 1px 2px rgba(15, 23, 42, 0.03));
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
}

.footer-zone {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
}

.footer-zone--primary {
    justify-content: flex-end;
    margin-left: auto;
}

/* 底栏按钮：同高、同宽、同内边距 */
:deep(.network-footer-btn.p-button) {
    min-width: clamp(0px, var(--et-btn-w, 10rem), 22vw);
    height: var(--et-btn, 2.5rem) !important;
    padding: 0 0.9rem !important;
    font-size: var(--et-fs-body, 0.875rem) !important;
    font-weight: 600 !important;
    border-radius: var(--et-radius, 0.5rem) !important;
    box-sizing: border-box;
}

:deep(.header-action-btn.p-button) {
    height: var(--et-btn, 2.5rem) !important;
    min-height: var(--et-btn, 2.5rem) !important;
    padding: 0 0.9rem !important;
    font-size: var(--et-fs-body, 0.875rem) !important;
    font-weight: 600 !important;
    border-radius: var(--et-radius, 0.5rem) !important;
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
    min-width: clamp(0px, var(--et-btn-w, 10rem), 22vw);
    padding: 0 0.9rem !important;
    font-size: var(--et-fs-body, 0.875rem) !important;
    font-weight: 600 !important;
    border-radius: var(--et-radius, 0.5rem) !important;
    box-sizing: border-box;
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
    border-left: 1px solid var(--surface-border, #e5e7eb);
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

    .toolbar-zone--network {
        padding-top: 0.5rem;
        border-top: 1px solid var(--surface-border, #e5e7eb);
    }

    .toolbar-zone-actions {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(0, 1fr));
        /* 有文案折行时让同一行按钮等高，避免参差不齐 */
        align-items: stretch;
        gap: 0.3rem;
    }

    .network-sticky-footer {
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(0, 1fr));
        align-items: stretch;
        gap: 0.3rem;
    }

    /* 让底栏主操作区里的按钮直接成为网格项，和「返回」共占一行 */
    .footer-zone,
    .footer-zone--primary {
        display: contents;
    }

    :deep(.network-footer-btn.p-button),
    :deep(.config-toolbar-btn.p-button) {
        min-width: 0;
        width: 100%;
        /* 窄屏文案可能折行，配合 auto 高度避免被裁掉 */
        height: auto !important;
        min-height: var(--et-btn, 2.5rem) !important;
        padding: 0.3rem !important;
        /* 12px：360px 屏下 4 列每列 75.1px，最长文案「编辑为文件」需 69.6px，留 5.5px 余量不折行。
           13px 时需 74.6px，只差 0.1px，太贴边。 */
        font-size: 0.75rem !important;
    }

    :deep(.config-toolbar-btn.p-button .p-button-icon),
    :deep(.network-footer-btn.p-button .p-button-icon) {
        display: none;
    }

    :deep(.config-toolbar-btn.p-button .p-button-label),
    :deep(.network-footer-btn.p-button .p-button-label) {
        white-space: normal;
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
    color: var(--primary-color, #3b82f6);
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
    .network-toolbar,
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
    .network-header,
    .network-toolbar {
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
