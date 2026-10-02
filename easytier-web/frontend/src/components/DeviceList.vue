<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { TOAST_LIFE } from 'easytier-frontend-lib'
import { Button, Dialog, InputText, ProgressSpinner, useToast, Dropdown } from 'primevue';
import { Utils, tooltipDirective } from 'easytier-frontend-lib';
import { useRouter } from 'vue-router';
import DeviceDetails from './DeviceDetails.vue';
import { useI18n } from 'vue-i18n'
import ApiClient from '../modules/api';
import { loadMergedDevices } from '../modules/deviceArchive';
import { usePollingList } from '../modules/usePollingList';

const { t } = useI18n()

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

const renameVisible = ref(false);
const renameSaving = ref(false);
const renameDeviceId = ref('');
const renameInput = ref('');
const renameReportedHostname = ref('');

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
            detail: String(e),
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

/** 打开设备管理全页：status=查看/启停；config=编辑/新建 */
const handleDeviceManagement = (device: Utils.DeviceInfo, mode: 'status' | 'config') => {
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
        let result = 0;

        switch (sortField) {
            case 'hostname':
                result = a.hostname.localeCompare(b.hostname);
                break;
            case 'version':
                result = a.easytier_version.localeCompare(b.easytier_version);
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

.device-card-actions {
    flex-shrink: 0;
    align-items: center;
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
    display: inline-flex;
    align-items: center;
    gap: var(--et-space-1);
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

                            <div class="text-xs version-badge" v-tooltip.top="`EasyTier ${device.easytier_version}`">
                                v{{ device.easytier_version.split('-')[0] }}
                            </div>
                        </div>

                        <div class="flex justify-between items-center">
                            <div class="text-sm truncate card-subtitle max-w-[60%] flex items-center gap-2"
                                v-tooltip.top="locationText(device)">
                                <i class="pi pi-map-marker location-icon"></i>
                                <span class="location-text">
                                    <template v-for="(part, index) in locationParts(device)" :key="index">
                                        <span v-if="index > 0" class="location-separator">·</span>
                                        {{ part }}
                                    </template>
                                    <template v-if="!locationParts(device).length">
                                        {{ t('web.device.unknown_location') }}
                                    </template>
                                </span>
                            </div>

                            <div class="device-card-actions flex items-center gap-2">
                                <!-- 运行中虚拟网数量（悬停样式与按钮一致） -->
                                <span
                                    class="et-icon-action et-icon-action--primary device-count-badge"
                                    v-tooltip.top="t('web.device.network_count')"
                                    :aria-label="`${t('web.device.network_count')}: ${device.running_network_count}`">
                                    {{ device.running_network_count }}
                                </span>

                                <Button v-tooltip.top="t('web.device.open_network_status')"
                                    icon="pi pi-chart-line" severity="info" rounded text
                                    class="et-icon-action-btn device-action-btn"
                                    @click="handleDeviceManagement(device, 'status')"
                                    :aria-label="t('web.device.open_network_status')" />

                                <Button v-tooltip.top="t('web.device.open_network_config')"
                                    icon="pi pi-cog" severity="secondary" rounded text
                                    class="et-icon-action-btn device-action-btn"
                                    @click="handleDeviceManagement(device, 'config')"
                                    :aria-label="t('web.device.open_network_config')" />
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
</template>
