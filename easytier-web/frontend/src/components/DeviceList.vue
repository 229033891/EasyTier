<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { TOAST_LIFE } from 'easytier-frontend-lib'
import { Button, Dialog, InputText, ProgressSpinner, useConfirm, useToast, Dropdown } from 'primevue';
import { Utils, tooltipDirective } from 'easytier-frontend-lib';
import { useRouter } from 'vue-router';
import DeviceDetails from './DeviceDetails.vue';
import { useI18n } from 'vue-i18n'
import ApiClient from '../modules/api';
import { loadMergedDevices } from '../modules/deviceArchive';
import { usePollingList } from '../modules/usePollingList';

const { t } = useI18n()
const errorDetail = (error: unknown) => Utils.formatApiErrorDetail(error, t)

declare const window: Window & typeof globalThis;

const vTooltip = tooltipDirective;

const props = defineProps({
    api: ApiClient,
});

// 从 localStorage 读取显示详情状态，默认为 false
const showDetailedView = ref(localStorage.getItem('deviceList.showDetailedView') === 'true');

// 监听显示详情状态变化，保存到 localStorage
watch(showDetailedView, (newValue) => {
    localStorage.setItem('deviceList.showDetailedView', newValue.toString());
});

const api = props.api;

const router = useRouter();
const toast = useToast();
const confirm = useConfirm();

const renameVisible = ref(false);
const renameSaving = ref(false);
const renameDeviceId = ref('');
const renameInput = ref('');
const renameReportedHostname = ref('');

const mergeVisible = ref(false);
const mergeSaving = ref(false);
const mergeSourceId = ref('');
const mergeSourceLabel = ref('');
const mergeTargetId = ref<string | null>(null);

const loadDevices = async (): Promise<Array<Utils.DeviceInfo>> => {
    return loadMergedDevices(api, { toast, t });
};

const { data: deviceList, reload: reloadDevices } = usePollingList<Array<Utils.DeviceInfo>>({
    fetcher: loadDevices,
});

const openRenameDialog = (device: Utils.DeviceInfo) => {
    renameDeviceId.value = device.machine_id;
    renameReportedHostname.value = device.reported_hostname || device.hostname;
    renameInput.value = (device.display_name || '').trim();
    renameVisible.value = true;
};

const saveRename = async () => {
    if (!api || !renameDeviceId.value || renameSaving.value) return;
    renameSaving.value = true;
    try {
        await api.update_device_display_name(renameDeviceId.value, renameInput.value.trim());
        renameVisible.value = false;
        await reloadDevices();
        toast.add({
            severity: 'success',
            summary: t('web.device.rename_success'),
            life: TOAST_LIFE.success,
        });
    } catch (e) {
        toast.add({
            severity: 'error',
            summary: t('web.device.rename_failed'),
            detail: errorDetail(e),
            life: TOAST_LIFE.error,
        });
    } finally {
        renameSaving.value = false;
    }
};

const clearRename = async () => {
    renameInput.value = '';
    await saveRename();
};

/** True when present in live list_machines; archive-only rows are offline. */
const isDeviceOnline = (device: Utils.DeviceInfo) => device.online !== false;

const mergeTargetOptions = computed(() => {
    const sourceId = mergeSourceId.value;
    return (deviceList.value || [])
        .filter((d) => d.machine_id && d.machine_id !== sourceId)
        .map((d) => ({
            label: `${d.hostname}${isDeviceOnline(d) ? '' : ` (${t('web.device.offline')})`}`,
            value: d.machine_id,
        }));
});

const openMergeDialog = (device: Utils.DeviceInfo) => {
    if (isDeviceOnline(device)) {
        toast.add({
            severity: 'warn',
            summary: t('web.device.merge'),
            detail: t('web.device.merge_online_denied'),
            life: TOAST_LIFE.warn,
        });
        return;
    }
    mergeSourceId.value = device.machine_id;
    mergeSourceLabel.value = device.hostname;
    mergeTargetId.value = null;
    const options = (deviceList.value || []).filter(
        (d) => d.machine_id && d.machine_id !== device.machine_id,
    );
    if (!options.length) {
        toast.add({
            severity: 'info',
            summary: t('web.device.merge'),
            detail: t('web.device.merge_no_targets'),
            life: TOAST_LIFE.info,
        });
        return;
    }
    // Prefer an online device with the same reported hostname when present.
    const reported = (device.reported_hostname || device.hostname || '').toLowerCase();
    const preferred =
        options.find(
            (d) =>
                isDeviceOnline(d) &&
                (d.reported_hostname || d.hostname || '').toLowerCase() === reported,
        ) ||
        options.find((d) => isDeviceOnline(d)) ||
        options[0];
    mergeTargetId.value = preferred?.machine_id ?? null;
    mergeVisible.value = true;
};

const confirmMerge = () => {
    if (!api || !mergeSourceId.value || !mergeTargetId.value || mergeSaving.value) return;
    const target = (deviceList.value || []).find((d) => d.machine_id === mergeTargetId.value);
    const targetLabel = target?.hostname || mergeTargetId.value;
    confirm.require({
        message: t('web.device.merge_confirm', [mergeSourceLabel.value, targetLabel]),
        header: t('web.device.merge_title'),
        icon: 'pi pi-sitemap',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.device.merge'),
            severity: 'warning',
        },
        accept: async () => {
            mergeSaving.value = true;
            try {
                await api.merge_devices(mergeSourceId.value, mergeTargetId.value!);
                mergeVisible.value = false;
                await reloadDevices();
                toast.add({
                    severity: 'success',
                    summary: t('web.device.merge_success'),
                    life: TOAST_LIFE.success,
                });
            } catch (e: any) {
                toast.add({
                    severity: 'error',
                    summary: t('web.device.merge_failed'),
                    detail: errorDetail(e),
                    life: TOAST_LIFE.error,
                });
            } finally {
                mergeSaving.value = false;
            }
        },
    });
};

const confirmRetire = (device: Utils.DeviceInfo) => {
    if (isDeviceOnline(device)) {
        toast.add({
            severity: 'warn',
            summary: t('web.device.retire'),
            detail: t('web.device.retire_online_denied'),
            life: TOAST_LIFE.warn,
        });
        return;
    }
    confirm.require({
        message: t('web.device.retire_confirm', [device.hostname]),
        header: t('web.device.retire'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
            label: t('web.common.cancel'),
            severity: 'secondary',
            outlined: true,
        },
        acceptProps: {
            label: t('web.device.retire'),
            severity: 'danger',
        },
        accept: async () => {
            if (!api) return;
            try {
                await api.delete_device(device.machine_id);
                await reloadDevices();
                toast.add({
                    severity: 'success',
                    summary: t('web.device.retire_success'),
                    life: TOAST_LIFE.success,
                });
            } catch (e: any) {
                toast.add({
                    severity: 'error',
                    summary: t('web.device.retire_failed'),
                    detail: errorDetail(e),
                    life: TOAST_LIFE.error,
                });
            }
        },
    });
};

/** 打开设备管理全页：status=查看/启停；config=编辑/新建。离线设备不可操作网络。 */
const handleDeviceManagement = (device: Utils.DeviceInfo, mode: 'status' | 'config') => {
    if (!isDeviceOnline(device)) {
        toast.add({
            severity: 'warn',
            summary: t('web.device.offline'),
            detail: t('web.device.offline_action_denied'),
            life: TOAST_LIFE.warn,
        });
        return;
    }
    const instanceId = device.running_network_instances?.[0];
    if (mode === 'status' && !instanceId) {
        toast.add({
            severity: 'info',
            summary: t('web.device.open_network_status'),
            detail: t('web.device.no_running_network_hint'),
            life: TOAST_LIFE.info,
        });
    }
    router.push({
        name: 'deviceManagement',
        params: {
            deviceId: device.machine_id,
            instanceId: instanceId
        },
        query: { mode, from: 'deviceList' },
    });
};

// 排序相关
const sortOptions = ref([
    { name: () => t('web.device.sort_by_hostname'), value: 'hostname', icon: 'pi pi-home' },
    { name: () => t('web.device.sort_by_version'), value: 'version', icon: 'pi pi-tag' },
    { name: () => t('web.device.sort_by_networks'), value: 'networks', icon: 'pi pi-sitemap' }
]);
const selectedSortOption = ref(sortOptions.value[0]);
// 排序方向 (true为升序，false为降序)
const ascending = ref(true);

// 切换排序方向
const toggleSortDirection = () => {
    ascending.value = !ascending.value;
};

// 排序函数
const sortDevices = (devices: Array<Utils.DeviceInfo> | undefined) => {
    if (!devices) return [];

    const sortField = selectedSortOption.value.value;
    const direction = ascending.value ? 1 : -1;

    return [...devices].sort((a, b) => {
        // 在线优先，避免离线设备挤在前面
        const onlineDiff = Number(isDeviceOnline(b)) - Number(isDeviceOnline(a));
        if (onlineDiff !== 0) {
            return onlineDiff;
        }

        let result = 0;

        switch (sortField) {
            case 'hostname':
                result = a.hostname.localeCompare(b.hostname);
                break;
            case 'version':
                result = (a.easytier_version || '').localeCompare(b.easytier_version || '');
                break;
            case 'networks':
                result = a.running_network_count - b.running_network_count;
                break;
        }

        return result * direction;
    });
};

// 排序后的设备列表
const sortedDeviceList = computed(() => {
    return sortDevices(deviceList.value);
});

/** 位置各段（国家 / 地区 / 城市），空值已过滤 */
const locationParts = (device: Utils.DeviceInfo): string[] => {
    const loc = device.location;
    if (!loc) return [];
    return [loc.country, loc.region, loc.city].filter((part): part is string => !!part);
};

/** 位置文案：无位置信息时回落到未知 */
const locationText = (device: Utils.DeviceInfo): string => {
    const parts = locationParts(device);
    return parts.length ? parts.join(' · ') : t('web.device.unknown_location');
};

</script>

<style scoped>
/* 卡片容器 */
.card-container {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: var(--et-gap-section);
    width: 100%;
    position: relative;
}

/* 设备卡片样式 */
.device-card {
    border: var(--et-border);
    border-radius: var(--et-radius);
    background: var(--surface-card, #ffffff);
    box-shadow: var(--et-shadow-card);
    transition: box-shadow 0.2s ease, background-color 0.3s ease, border-color 0.2s ease;
    display: flex;
    flex-direction: column;
    position: relative;
    overflow: hidden;
}

/* hover 放进 @media (hover: hover)：触屏点一下后不会「粘住」高亮 */
@media (hover: hover) {
    .device-card:hover {
        /* 不用 translateY，避免与 tooltip 抢焦点导致闪烁 */
        box-shadow: 0 4px 6px -1px rgba(0, 0, 0, 0.1), 0 2px 4px -1px rgba(0, 0, 0, 0.06);
        border-color: var(--primary-color, var(--et-primary, #0ea5e9));
    }
}

.card-header {
    padding: var(--et-pad-card);
    display: flex;
    flex-direction: column;
    position: relative;
    color: var(--text-color, #1e293b);
}

.card-details {
    background-color: var(--surface-ground, #f8fafc);
}

:deep(.card-details-content) {
    padding: var(--et-space-1);
}

:deep(.card-details-content .detail-label) {
    font-size: var(--et-fs-body);
}

:deep(.card-details-content .detail-value) {
    font-size: var(--et-fs-meta);
}

@media (prefers-color-scheme: dark) {
    :deep(.card-details-content .detail-item) {
        border-bottom: 1px solid var(--surface-border, #334155);
    }

    :deep(.card-details-content .detail-item:last-child) {
        border-bottom: none;
    }

    @media (hover: hover) {
        :deep(.card-details-content .detail-item:hover) {
            background-color: var(--surface-hover, rgba(30, 41, 59, 0.4));
        }
    }

    :deep(.card-details-content .detail-label) {
        color: var(--text-color, #e2e8f0);
    }

    :deep(.card-details-content .detail-value) {
        color: var(--text-color-secondary, #cbd5e1);
    }
}

.card-title {
    color: var(--text-color, #1e293b);
    font-size: var(--et-fs-section);
    font-weight: 600;
}

.card-subtitle {
    color: var(--text-color-secondary, #64748b);
}

.version-badge {
    background-color: var(--primary-color, var(--et-primary, #0ea5e9));
    color: #ffffff;
    padding: 0.1rem 0.4rem;
    border-radius: 999px;
    font-weight: 500;
    letter-spacing: 0.02em;
    font-size: var(--et-fs-meta);
}

.device-status-dot {
    flex: 0 0 auto;
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 999px;
}

.device-status-dot--online {
    background: var(--et-success, #10b981);
}

.device-status-dot--offline {
    background: var(--text-color-secondary, #94a3b8);
}

.device-status-tag {
    font-size: var(--et-fs-meta, 0.75rem);
    font-weight: 600;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
    line-height: 1.2;
    white-space: nowrap;
}

.device-status-tag--online {
    color: #047857;
    background: color-mix(in srgb, var(--et-success, #10b981) 16%, transparent);
}

.device-status-tag--offline {
    color: var(--text-color-secondary, #64748b);
    background: var(--surface-100, #f1f5f9);
}

.sort-dropdown {
    min-width: 6rem;
    max-width: 9rem;
}

.sort-direction-btn {
    font-size: 0.875rem;
}

:deep(.p-button.p-button-icon-only.sort-direction-btn) {
    width: 2rem !important;
    height: 2rem !important;
    min-width: 2rem !important;
}

@media (prefers-color-scheme: dark) {
    .device-card {
        background-color: var(--surface-card, #1e293b);
        border-color: var(--surface-border, #334155);
        box-shadow: 0 1px 3px 0 rgba(0, 0, 0, 0.3);
    }

    .card-header {
        color: var(--text-color, #f1f5f9);
    }

    .card-title {
        color: var(--text-color, #f1f5f9);
    }

    .card-subtitle {
        color: var(--text-color-secondary, #cbd5e1);
    }

    .version-badge {
        background-color: var(--primary-color, var(--et-primary, #0ea5e9));
    }

    .device-status-tag--online {
        color: #6ee7b7;
        background: color-mix(in srgb, var(--et-success, #10b981) 22%, transparent);
    }

    .device-status-tag--offline {
        color: var(--text-color-secondary, #94a3b8);
        background: var(--surface-100, #334155);
    }

    .card-details {
        background-color: var(--surface-ground, #0f172a);
        border-top: 1px solid var(--surface-border, #334155);
    }
}

@media (max-width: 768px) {
    .card-container {
        grid-template-columns: 1fr;
    }
}

@keyframes fadeIn {
    from {
        opacity: 0;
        transform: translateY(-10px);
    }

    to {
        opacity: 1;
        transform: translateY(0);
    }
}

.fade-in {
    animation: fadeIn 0.3s ease-out;
}

:deep(.p-dropdown) {
    background: transparent;
    border: var(--et-border);
    /* 不用 transition: all —— 只过渡实际会变的属性（悬停改边框色、聚焦加 ring） */
    transition: border-color 0.2s ease, box-shadow 0.2s ease, background-color 0.2s ease;
}

@media (hover: hover) {
    :deep(.p-dropdown:hover) {
        border-color: var(--primary-color);
    }
}

.device-card-meta-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem 0.75rem;
    min-width: 0;
}

.device-card-meta {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
    flex: 1 1 auto;
    font-size: var(--et-fs-body, 0.875rem);
}

.device-card-actions {
    display: flex;
    flex-shrink: 0;
    align-items: center;
    gap: 0.5rem;
}

/* Narrow screens: stack meta text and action buttons to avoid overlap. */
@media (max-width: 480px) {
    .device-card-meta-row {
        flex-wrap: wrap;
    }

    .device-card-meta {
        flex: 1 1 100%;
        max-width: 100%;
    }

    .device-card-actions {
        margin-left: auto;
    }
}

.device-count-badge {
    cursor: default;
    background: color-mix(in srgb, #10b981 12%, #ffffff) !important;
    color: #047857 !important;
    border-color: color-mix(in srgb, #10b981 28%, transparent) !important;
}

@media (prefers-color-scheme: dark) {
    .device-count-badge {
        background: color-mix(in srgb, #10b981 22%, transparent) !important;
        color: #34d399 !important;
    }
}

.location-icon {
    color: var(--primary-color, var(--et-primary, #0ea5e9));
    font-size: var(--et-fs-body);
}

.location-text {
    font-size: var(--et-fs-body);
    line-height: 1.25rem;
    opacity: 0.9;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}

.location-separator {
    opacity: 0.5;
    font-weight: 300;
    margin: 0 0.1rem;
}

@media (prefers-color-scheme: dark) {
    .location-text {
        color: var(--text-color-secondary, #cbd5e1);
    }

    .location-icon {
        color: var(--primary-color, var(--et-primary, #0ea5e9));
    }
}
/* 工具条：轻量一行，不做卡片壳 */
.device-list-toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem 1.25rem;
    padding: 0.125rem 0.125rem 0.25rem;
}

.device-list-toolbar-group {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    min-width: 0;
}

.device-list-toolbar label {
    color: var(--text-color-secondary, #64748b);
    white-space: nowrap;
}

.detailed-view-btn {
    min-height: var(--et-btn-sm, 2.25rem) !important;
    height: var(--et-btn-sm, 2.25rem) !important;
    padding-inline: 0.85rem !important;
    font-weight: 600;
    white-space: nowrap;
}

@media (max-width: 540px) {
    .device-list-toolbar {
        flex-direction: column;
        align-items: stretch;
    }

    .device-list-toolbar-group {
        width: 100%;
    }

    .device-list-toolbar-group .sort-dropdown {
        flex: 1 1 auto;
        max-width: none;
        min-width: 0;
        width: 100%;
    }

    .detailed-view-btn {
        width: 100%;
        justify-content: center;
    }
}
</style>

<template>
    <div class="et-page">
        <div class="et-page-header">
            <h1 class="et-page-title">{{ t('web.device.list') }}</h1>
        </div>

        <div class="device-list-toolbar">
            <div class="device-list-toolbar-group">
                <label for="sort-by" class="text-sm hidden sm:block">{{ t('web.device.sort_by') }}</label>
                <Dropdown id="sort-by" v-model="selectedSortOption" :options="sortOptions" optionLabel="name"
                    class="sort-dropdown text-sm !min-w-[120px] sm:!min-w-[140px]" panelClass="text-sm">
                    <template #value="slotProps">
                        <div class="flex items-center gap-2">
                            <i :class="[slotProps.value.icon, 'text-muted-color']"></i>
                            <span class="text-color">{{ slotProps.value.name() }}</span>
                        </div>
                    </template>
                    <template #option="slotProps">
                        <div class="flex items-center gap-2">
                            <i :class="[slotProps.option.icon, 'text-muted-color']"></i>
                            <span>{{ slotProps.option.name() }}</span>
                        </div>
                    </template>
                </Dropdown>
                <Button :icon="ascending ? 'pi pi-sort-amount-up' : 'pi pi-sort-amount-down'" severity="secondary"
                    text rounded class="et-icon-action-btn sort-direction-btn"
                    v-tooltip.top="ascending ? t('web.device.sort_direction_asc') : t('web.device.sort_direction_desc')"
                    @click="toggleSortDirection" />
            </div>
            <div class="device-list-toolbar-group">
                <Button
                    class="detailed-view-btn"
                    :icon="showDetailedView ? 'pi pi-eye-slash' : 'pi pi-eye'"
                    :label="showDetailedView ? t('web.device.hide_detailed_view') : t('web.device.show_detailed_view')"
                    :severity="showDetailedView ? 'info' : 'secondary'"
                    :outlined="!showDetailedView"
                    size="small"
                    :aria-pressed="showDetailedView"
                    @click="showDetailedView = !showDetailedView"
                />
            </div>
        </div>

        <div v-if="deviceList === undefined" class="w-full flex justify-center">
            <ProgressSpinner />
        </div>

        <div v-else class="card-container">
                <div v-for="device in sortedDeviceList" :key="device.machine_id" class="device-card">
                    <div class="card-header">
                        <div class="flex justify-between items-center mb-2">
                            <div class="flex items-center gap-1 min-w-0 flex-1">
                                <span
                                    class="device-status-dot"
                                    :class="isDeviceOnline(device) ? 'device-status-dot--online' : 'device-status-dot--offline'"
                                    :title="isDeviceOnline(device) ? t('web.device.online') : t('web.device.offline')"
                                    :aria-label="isDeviceOnline(device) ? t('web.device.online') : t('web.device.offline')"
                                />
                                <div class="font-semibold truncate card-title"
                                    v-tooltip.top="device.reported_hostname && device.reported_hostname !== device.hostname
                                        ? `${device.hostname} (${device.reported_hostname})`
                                        : device.hostname">{{ device.hostname }}
                                </div>
                                <Button
                                    icon="pi pi-pencil"
                                    severity="secondary"
                                    rounded
                                    text
                                    size="small"
                                    class="et-icon-action-btn device-rename-btn shrink-0"
                                    v-tooltip.top="t('web.device.rename')"
                                    :aria-label="t('web.device.rename')"
                                    @click="openRenameDialog(device)"
                                />
                            </div>

                            <div class="flex items-center gap-2 shrink-0">
                                <span
                                    class="device-status-tag"
                                    :class="isDeviceOnline(device) ? 'device-status-tag--online' : 'device-status-tag--offline'"
                                >
                                    {{ isDeviceOnline(device) ? t('web.device.online') : t('web.device.offline') }}
                                </span>
                                <div class="text-xs version-badge" v-tooltip.top="`EasyTier ${device.easytier_version || '—'}`">
                                    v{{ (device.easytier_version || '').split('-')[0] || '—' }}
                                </div>
                            </div>
                        </div>

                        <div class="device-card-meta-row">
                            <div class="card-subtitle device-card-meta"
                                v-tooltip.top="isDeviceOnline(device)
                                    ? locationText(device)
                                    : (device.report_time
                                        ? `${t('web.device.last_seen')}: ${device.report_time}`
                                        : t('web.device.offline'))">
                                <i class="pi pi-map-marker location-icon"></i>
                                <span class="location-text">
                                    <template v-if="!isDeviceOnline(device)">
                                        {{ device.report_time
                                            ? `${t('web.device.last_seen')}: ${device.report_time}`
                                            : t('web.device.offline') }}
                                    </template>
                                    <template v-else>
                                        <template v-for="(part, index) in locationParts(device)" :key="index">
                                            <span v-if="index > 0" class="location-separator">·</span>
                                            {{ part }}
                                        </template>
                                        <template v-if="!locationParts(device).length">
                                            {{ t('web.device.unknown_location') }}
                                        </template>
                                    </template>
                                </span>
                            </div>

                            <div class="device-card-actions">
                                <!-- 运行中虚拟网数量（悬停样式与按钮一致） -->
                                <span
                                    class="et-icon-action et-icon-action--primary device-count-badge"
                                    v-tooltip.top="t('web.device.network_count')"
                                    :aria-label="`${t('web.device.network_count')}: ${device.running_network_count}`">
                                    {{ device.running_network_count }}
                                </span>

                                <Button
                                    v-tooltip.top="isDeviceOnline(device)
                                        ? t('web.device.open_network_status')
                                        : t('web.device.offline_action_denied')"
                                    icon="pi pi-chart-line" severity="info" rounded text
                                    class="et-icon-action-btn device-action-btn"
                                    :disabled="!isDeviceOnline(device)"
                                    @click="handleDeviceManagement(device, 'status')"
                                    :aria-label="t('web.device.open_network_status')" />

                                <Button
                                    v-tooltip.top="isDeviceOnline(device)
                                        ? t('web.device.open_network_config')
                                        : t('web.device.offline_action_denied')"
                                    icon="pi pi-cog" severity="secondary" rounded text
                                    class="et-icon-action-btn device-action-btn"
                                    :disabled="!isDeviceOnline(device)"
                                    @click="handleDeviceManagement(device, 'config')"
                                    :aria-label="t('web.device.open_network_config')" />

                                <Button
                                    v-if="!isDeviceOnline(device)"
                                    v-tooltip.top="t('web.device.merge')"
                                    icon="pi pi-arrow-right-arrow-left" severity="help" rounded text
                                    class="et-icon-action-btn device-action-btn"
                                    @click="openMergeDialog(device)"
                                    :aria-label="t('web.device.merge')" />

                                <Button
                                    v-if="!isDeviceOnline(device)"
                                    v-tooltip.top="t('web.device.retire')"
                                    icon="pi pi-trash" severity="danger" rounded text
                                    class="et-icon-action-btn device-action-btn"
                                    @click="confirmRetire(device)"
                                    :aria-label="t('web.device.retire')" />
                            </div>
                        </div>
                    </div>

                    <div v-if="showDetailedView" class="card-details border-t border-surface fade-in">
                        <DeviceDetails :device="device" containerClass="card-details-content" :compact="true" />
                    </div>
                </div>
        </div>
    </div>

    <Dialog
        v-model:visible="renameVisible"
        modal
        :header="t('web.device.rename')"
        :style="{ width: '24rem' }"
        :closable="!renameSaving"
    >
        <div class="flex flex-col gap-3">
            <div class="text-sm text-muted-color">
                {{ t('web.device.rename_reported', [renameReportedHostname]) }}
            </div>
            <div class="text-sm text-muted-color">
                {{ t('web.device.rename_hint') }}
            </div>
            <InputText
                v-model="renameInput"
                class="w-full"
                :placeholder="t('web.device.rename_placeholder')"
                maxlength="32"
                :disabled="renameSaving"
                @keyup.enter="saveRename"
            />
        </div>
        <template #footer>
            <Button
                :label="t('web.device.rename_clear')"
                severity="secondary"
                text
                :disabled="renameSaving"
                @click="clearRename"
            />
            <Button
                :label="t('close')"
                severity="secondary"
                text
                :disabled="renameSaving"
                @click="renameVisible = false"
            />
            <Button
                :label="t('save')"
                :loading="renameSaving"
                @click="saveRename"
            />
        </template>
    </Dialog>

    <Dialog
        v-model:visible="mergeVisible"
        modal
        :header="t('web.device.merge_title')"
        :style="{ width: '26rem' }"
        :closable="!mergeSaving"
    >
        <div class="flex flex-col gap-3">
            <div class="text-sm text-muted-color">
                {{ t('web.device.merge_hint') }}
            </div>
            <div class="text-sm">
                <span class="text-muted-color">{{ t('web.device.merge_source') }}:</span>
                <span class="font-semibold ml-1">{{ mergeSourceLabel }}</span>
            </div>
            <div class="flex flex-col gap-1">
                <label class="text-sm text-muted-color" for="merge-target">{{ t('web.device.merge_target') }}</label>
                <Dropdown
                    id="merge-target"
                    v-model="mergeTargetId"
                    :options="mergeTargetOptions"
                    optionLabel="label"
                    optionValue="value"
                    class="w-full"
                    :placeholder="t('web.device.merge_target_placeholder')"
                    :disabled="mergeSaving"
                />
            </div>
        </div>
        <template #footer>
            <Button
                :label="t('close')"
                severity="secondary"
                text
                :disabled="mergeSaving"
                @click="mergeVisible = false"
            />
            <Button
                :label="t('web.device.merge')"
                severity="warning"
                :loading="mergeSaving"
                :disabled="!mergeTargetId"
                @click="confirmMerge"
            />
        </template>
    </Dialog>
</template>
