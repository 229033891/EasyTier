<script setup lang="ts">
import { Button, ConfirmDialog, ConfirmPopup, Divider, Menu, Message, Select, Tag, useConfirm, useToast, type VirtualScrollerLazyEvent } from 'primevue';
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

const configFile = ref();

const curNetworkInfo = ref<NetworkTypes.NetworkInstance | null>(null);

const showConfigEditDialog = ref(false);
const isEditingNetwork = ref(false); // Flag to indicate if we're in network editing mode
const currentNetworkConfig = ref<NetworkTypes.NetworkConfig | undefined>(undefined);

const listInstanceIdResponse = ref<Api.ListNetworkInstanceIdResponse | undefined>(undefined);

const isRunning = (instanceId: string) => {
    return (listInstanceIdResponse.value?.running_inst_ids ?? []).map(Utils.UuidToStr).includes(instanceId);
}

const networkMetaCache = ref<Record<string, Api.NetworkMeta>>({});
const loadNetworkMetas = async (instanceIds: string[]) => {
    const missingIds = instanceIds.filter(id => !networkMetaCache.value[id]);

    if (missingIds.length === 0) return;

    try {
        const response = await props.api.get_network_metas(missingIds);
        Object.assign(networkMetaCache.value, response.metas ?? {});
    } catch (e) {
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
        return Api.ConfigFilePermission.isRemoveSaveable(currentNetworkMeta.value?.config_permission ?? 0);
    }),
    editable: computed(() => {
        return Api.ConfigFilePermission.isEditable(currentNetworkMeta.value?.config_permission ?? 0);
    }),
    deletable: computed(() => {
        return Api.ConfigFilePermission.isDeletable(currentNetworkMeta.value?.config_permission ?? 0);
    })
}

/** 配置页常显保存：有配置且可编辑（运行中也可只存盘不重跑） */
const canSaveConfig = computed(() => {
    if (!currentNetworkConfig.value) {
        return false;
    }
    // 新建/尚无 meta 时允许保存；已有权限则以 editable 为准
    if (!currentNetworkMeta.value) {
        return true;
    }
    return currentNetworkControl.editable.value;
});

const savingConfig = ref(false);

const instanceList = ref<Array<{ uuid: string; meta?: Api.NetworkMeta }>>([]);
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

    if (JSON.stringify(newList) !== JSON.stringify(instanceList.value)) {
        instanceList.value = newList;
    }
}
watch(listInstanceIdResponse, updateInstanceList, { deep: false });
watch(networkMetaCache, updateInstanceList, { deep: true });
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
    if (newVal?.length && !instanceId.value) {
        const running = newVal.find(item => isRunning(item.uuid));
        instanceId.value = (running ?? newVal[0]).uuid;
    }
});

const selectedInstanceId = computed({
    get() {
        return instanceList.value.find((instance) => instance.uuid === instanceId.value);
    },
    set(value: any) {
        console.log("set instanceId", value);
        instanceId.value = value ? value.uuid : undefined;
    }
});
watch(selectedInstanceId, async (newVal, oldVal) => {
    if (newVal?.uuid !== oldVal?.uuid && (networkIsDisabled.value || isEditingNetwork.value)) {
        await loadCurrentNetworkConfig();
    } else {
        await loadCurrentNetworkInfo();
    }

    if (newVal?.uuid && !networkMetaCache.value[newVal.uuid]) {
        await loadNetworkMetas([newVal.uuid]);
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
    return (listInstanceIdResponse.value?.disabled_inst_ids ?? []).map(Utils.UuidToStr).includes(selectedInstanceId.value?.uuid);
});
watch(networkIsDisabled, async (newVal, oldVal) => {
    if (newVal !== oldVal && newVal === true) {
        await loadCurrentNetworkConfig();
    }
});

const loadCurrentNetworkConfig = async () => {
    currentNetworkConfig.value = undefined;

    if (!selectedInstanceId.value) {
        return;
    }

    let ret = await props.api.get_network_config(selectedInstanceId.value!.uuid);
    currentNetworkConfig.value = ret;
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
            summary: 'Error',
            detail: 'Failed to start network, error: ' + JSON.stringify(e.response?.data ?? e),
            life: 2000,
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
        currentNetworkConfig.value = await props.api.get_network_config(selectedInstanceId.value.uuid);
        isEditingNetwork.value = true;
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
        toast.add({ severity: 'error', summary: 'Error', detail: 'Failed to run network, error: ' + JSON.stringify(e.response?.data ?? e), life: 2000 });
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
            detail: t('web.device_management.save_failed') + ': ' + JSON.stringify(e.response?.data ?? e),
            life: 3000,
        });
    } finally {
        savingConfig.value = false;
    }
}
const newNetwork = async () => {
    const newNetworkConfig = props.newConfigGenerator?.() ?? NetworkTypes.DEFAULT_NETWORK_CONFIG();
    await props.api.save_config(newNetworkConfig);
    selectedInstanceId.value = { uuid: newNetworkConfig.instance_id };
    currentNetworkConfig.value = newNetworkConfig;
    await loadNetworkInstanceIds();
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
        let ret = await props.api.get_network_config(instanceId.value!);
        console.debug("editNetwork", ret);
        currentNetworkConfig.value = ret;
        isEditingNetwork.value = true; // Switch to editing mode instead
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: 'Error', detail: 'Failed to edit network, error: ' + JSON.stringify(e.response.data), life: 2000 });
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

    try {
        const { instance_id, ...networkConfig } = await props.api.get_network_config(instanceId.value!);
        let { toml_config: tomlConfig, error } = await props.api.generate_config(networkConfig as NetworkTypes.NetworkConfig);
        if (error) {
            throw { response: { data: error } };
        }
        exportTomlFile(tomlConfig ?? '', instanceId.value + '.toml');
    } catch (e: any) {
        console.error(e);
        toast.add({ severity: 'error', summary: 'Error', detail: 'Failed to export network config, error: ' + JSON.stringify(e.response.data), life: 2000 });
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

            config.instance_id = currentNetworkConfig.value?.instance_id ?? config?.instance_id;
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
    let resp = await props.api.parse_config(tomlConfig);
    if (resp.error) {
        throw resp.error;
    };
    const config = resp.config;
    if (!config) {
        throw new Error("Parsed config is empty");
    }
    config.instance_id = currentNetworkConfig.value?.instance_id ?? config?.instance_id;
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
                    <span class="network-label">{{ t('web.device_management.network') }}</span>
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

        <!-- Main Content Area -->
        <div class="network-content">
            <Message v-if="showStatusDisabledPanel" severity="warn" class="mb-0">
                {{ t('web.device_management.network_disabled_hint') }}
            </Message>

            <template v-else-if="showConfigPanel">
                <div class="config-toolbar">
                    <div class="toolbar-zone">
                        <span class="toolbar-zone-label">{{ t('web.device_management.toolbar_config_files') }}</span>
                        <div class="toolbar-zone-actions">
                            <Button class="config-toolbar-btn" @click="showConfigEditDialog = true" icon="pi pi-file-edit"
                                :label="t('web.device_management.edit_as_file')" iconPos="left" severity="secondary" />
                            <Button class="config-toolbar-btn" @click="importConfig" icon="pi pi-upload"
                                :label="t('web.device_management.import_config')" iconPos="left" severity="secondary" />
                            <Button v-if="selectedInstanceId" class="config-toolbar-btn" @click="exportConfig" icon="pi pi-download"
                                :label="t('web.device_management.export_config')" iconPos="left" severity="secondary" />
                            <Button v-if="canSaveConfig" class="config-toolbar-btn" @click="saveNetworkConfig"
                                :disabled="!currentNetworkConfig || savingConfig"
                                icon="pi pi-save" :label="t('web.device_management.save_config')" iconPos="left"
                                severity="success" />
                        </div>
                    </div>
                    <div class="toolbar-zone toolbar-zone--network">
                        <span class="toolbar-zone-label">{{ t('web.device_management.toolbar_network') }}</span>
                        <div class="toolbar-zone-actions">
                            <Button class="config-toolbar-btn" @click="newNetwork" icon="pi pi-plus"
                                :label="t('web.device_management.add_network')" iconPos="left" severity="success" />
                            <Button v-if="selectedInstanceId && currentNetworkControl.deletable.value"
                                class="config-toolbar-btn" @click="confirmDeleteNetwork" icon="pi pi-trash"
                                :label="t('web.device_management.delete_network')" iconPos="left" severity="danger"
                                outlined />
                        </div>
                    </div>
                </div>

                <Divider />

                <Config :cur-network="currentNetworkConfig" :config-invalid="!currentNetworkConfig"
                    :hide-run-button="true" @run-network="saveAndRunNewNetwork"></Config>
            </template>

            <template v-else-if="needShowNetworkStatus">
                <Status v-if="curNetworkInfo && curNetworkInfo.error_msg === ''" v-bind:cur-network-inst="curNetworkInfo"
                    :api="api" class="mb-0" />
                <Message v-else-if="curNetworkInfo?.error_msg" severity="error" class="mb-0">{{
                    curNetworkInfo.error_msg }}</Message>
                <Message v-else severity="info" class="mb-0">{{ t('web.device_management.loading_network_status') }}
                </Message>
            </template>

            <div v-else class="empty-state flex flex-col items-center py-12">
                <i class="pi pi-sitemap text-5xl text-secondary mb-4 opacity-50"></i>
                <div class="text-xl text-center font-medium mb-3">{{ t('web.device_management.no_network_selected') }}
                </div>
                <p class="text-secondary text-center mb-6 max-w-md">
                    {{ isStatusMode
                        ? t('web.device_management.select_network_for_status')
                        : t('web.device_management.select_existing_network_or_create_new') }}
                </p>
                <Button v-if="!isStatusMode" @click="newNetwork"
                    :label="t('web.device_management.add_network')" icon="pi pi-plus" iconPos="left" />
                <Button v-else @click="requestSwitchMode('config')"
                    :label="t('web.device_management.switch_to_config')" icon="pi pi-cog" iconPos="left"
                    severity="secondary" />
            </div>
        </div>

        <div v-if="showStickyFooter" class="network-sticky-footer">
            <Button v-if="showLeaveInFooter" @click="drawerClose" :label="leaveLabel" severity="secondary"
                :icon="leaveIcon" iconPos="left" class="network-footer-btn" />
            <div class="footer-zone footer-zone--primary">
                <Button v-if="isConfigMode" icon="pi pi-chart-line" severity="secondary"
                    :label="t('web.device_management.switch_to_status')" iconPos="left"
                    class="network-footer-btn" @click="requestSwitchMode('status')" />
                <Button v-else-if="isStatusMode" icon="pi pi-cog" severity="secondary"
                    :label="t('web.device_management.switch_to_config')" iconPos="left"
                    class="network-footer-btn" @click="requestSwitchMode('config')" />

                <Button v-if="stickyFooterPrimary === 'start'" @click="confirmStartNetwork($event)"
                    :disabled="!currentNetworkControl.deletable.value" :label="t('web.network.start')"
                    severity="success" icon="pi pi-play" iconPos="left" class="network-footer-btn" />
                <Button v-else-if="stickyFooterPrimary === 'run'"
                    @click="saveAndRunNewNetwork(currentNetworkConfig!)" :disabled="!currentNetworkConfig"
                    :label="t('run_network')" severity="success" icon="pi pi-arrow-right" iconPos="right"
                    class="network-footer-btn" />
                <Button v-else-if="stickyFooterPrimary === 'stop'" @click="confirmStopNetwork($event)"
                    :disabled="!currentNetworkControl.deletable.value"
                    :label="t('web.device_management.disable_network')" severity="danger" icon="pi pi-power-off"
                    iconPos="left" class="network-footer-btn" />
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
    gap: 0.35rem;
}

.device-management--page {
    max-height: calc(100vh - 3.5rem);
}

.network-header {
    flex-shrink: 0;
    position: sticky;
    top: 0;
    z-index: 20;
    background: var(--surface-card, #ffffff) !important;
    border: var(--et-border, 1px solid var(--surface-border, #e5e7eb));
    border-radius: var(--et-radius, 0.5rem);
    box-shadow: none;
    padding: 0.4rem 0.75rem 0.5rem !important;
    margin: 0 !important;
}

.network-page-title {
    margin: 0 0 0.25rem;
    font-size: var(--et-fs-page-title, 1.25rem);
    font-weight: 700;
    line-height: 1.3;
}

.network-content {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
    padding: var(--et-pad-card, 0.75rem 1rem) !important;
    background: var(--surface-card, #ffffff) !important;
    border: var(--et-border, 1px solid var(--surface-border, #e5e7eb));
    border-radius: var(--et-radius, 0.5rem);
    box-shadow: none !important;
}

.network-sticky-footer {
    flex-shrink: 0;
    position: sticky;
    bottom: 0;
    z-index: 20;
    margin-top: 0;
    padding: var(--et-pad-card, 0.75rem 1rem);
    background: var(--surface-card, #ffffff);
    border: var(--et-border, 1px solid var(--surface-border, #e5e7eb));
    border-radius: var(--et-radius, 0.5rem);
    box-shadow: none;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
}

.network-status-actions {
    align-items: center;
}

.footer-zone {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
}

.footer-zone--primary {
    justify-content: flex-end;
    margin-left: auto;
}

/* 底栏按钮：同高、同最小宽、同内边距 */
:deep(.network-footer-btn.p-button) {
    min-width: 11.5rem;
    height: var(--et-btn, 2.5rem) !important;
    padding: 0 1rem !important;
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
    min-width: 8.75rem;
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
    gap: 1rem 1.5rem;
    margin-bottom: 0.25rem;
}

.toolbar-zone {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
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
    gap: 0.5rem;
}

.toolbar-zone--network {
    padding-left: 1rem;
    border-left: 1px solid var(--surface-border, #e5e7eb);
}

@media (max-width: 640px) {
    .toolbar-zone--network {
        padding-left: 0;
        border-left: none;
        padding-top: 0.5rem;
        border-top: 1px solid var(--surface-border, #e5e7eb);
        width: 100%;
    }

    .network-sticky-footer {
        flex-direction: column;
        align-items: stretch;
    }

    .footer-zone,
    .footer-zone--primary {
        margin-left: 0;
        justify-content: stretch;
        width: 100%;
    }

    :deep(.network-footer-btn.p-button),
    :deep(.config-toolbar-btn.p-button) {
        flex: 1 1 auto;
        min-width: 0;
        width: 100%;
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

:deep(.p-menu .p-menuitem.p-error .p-menuitem-text,
    .p-menu .p-menuitem.p-error .p-menuitem-icon) {
    color: var(--red-500);
}

:deep(.p-menu .p-menuitem:hover.p-error .p-menuitem-link) {
    background-color: var(--red-50);
}

/* 仅顶栏纯图标按钮，避免覆盖底栏带文字按钮 */
:deep(.header-action-btn.p-button-icon-only .p-button-icon) {
    font-size: 1rem;
    margin: 0;
}

/* 网络选择相关样式 */
.network-label {
    display: block;
    margin: 0 0 0.2rem;
    font-size: var(--et-fs-meta, 0.75rem);
    font-weight: 600;
    color: var(--text-color-secondary, #64748b);
    white-space: nowrap;
}

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
    .network-content,
    .network-sticky-footer {
        background: var(--surface-card, #1e293b) !important;
        border-color: var(--surface-border, #334155);
        box-shadow: none !important;
    }
}

/* Responsive design for mobile devices */
@media (max-width: 768px) {
    .device-management--page {
        max-height: calc(100vh - 6.5rem);
    }

    .network-header {
        padding: 0.75rem;
    }

    .network-content {
        padding: 0.75rem;
    }

    /* 在小屏幕上缩短网络标签文本 */
    .network-label {
        font-size: 0.9rem;
    }

    .network-footer-btn {
        min-width: 7.5rem;
    }
}
</style>
