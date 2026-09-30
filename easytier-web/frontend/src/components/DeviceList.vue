<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { Button, ProgressSpinner, useToast, InputSwitch, Dropdown, Toolbar } from 'primevue';
import { tooltipDirective } from '../modules/tooltip';
import { useRouter } from 'vue-router';
import { Utils } from 'easytier-frontend-lib';
import DeviceDetails from './DeviceDetails.vue';
import { useI18n } from 'vue-i18n'
import ApiClient from '../modules/api';
import { usePollingList } from '../modules/usePollingList';

const { t } = useI18n()

declare const window: Window & typeof globalThis;

// 注册 Tooltip 指令
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

const loadDevices = async (): Promise<Array<Utils.DeviceInfo>> => {
    const resp = await api?.list_machines();
    const devices: Array<Utils.DeviceInfo> = [];
    for (const device of (resp || [])) {
        devices.push(Utils.buildDeviceInfo(device));
    }
    return devices;
};

const { data: deviceList } = usePollingList<Array<Utils.DeviceInfo>>({ fetcher: loadDevices });

/** 打开设备管理全页：status=查看/启停；config=编辑/新建 */
const handleDeviceManagement = (device: Utils.DeviceInfo, mode: 'status' | 'config') => {
    const instanceId = device.running_network_instances?.[0];
    if (mode === 'status' && !instanceId) {
        toast.add({
            severity: 'info',
            summary: t('web.device.open_network_status'),
            detail: t('web.device.no_running_network_hint'),
            life: 3500,
        });
    }
    router.push({
        name: 'deviceManagement',
        params: {
            deviceId: device.machine_id,
            instanceId: instanceId
        },
        query: { mode },
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
    gap: 1rem;
    width: 100%;
    position: relative;
}

/* 设备卡片样式 */
.device-card {
    border: 1px solid var(--surface-border, #e5e7eb);
    border-radius: 0.5rem;
    background: var(--surface-card, white);
    box-shadow: 0 1px 3px 0 rgba(0, 0, 0, 0.1);
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
        border-color: var(--primary-color, #3b82f6);
    }
}

.card-header {
    padding: var(--et-pad-card);
    display: flex;
    flex-direction: column;
    position: relative;
    color: var(--text-color, #1f2937);
}

.card-details {
    background-color: var(--surface-ground, #f9fafb);
}

:deep(.card-details-content) {
    padding: 0.15rem 0.1rem;
}

:deep(.card-details-content .detail-label) {
    font-size: 0.9rem;
}

:deep(.card-details-content .detail-value) {
    font-size: 0.85rem;
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

:deep(.device-card) {
    background-color: var(--surface-card, white);
    border-color: var(--surface-border, #e5e7eb);
}

:deep(.card-header) {
    color: var(--text-color, #1f2937);
}

.card-title {
    color: var(--text-color, #1f2937);
}

.card-subtitle {
    color: var(--text-color-secondary, #64748b);
}

.version-badge {
    background-color: var(--primary-color, #0ea5e9);
    color: #ffffff;
    padding: 0.1rem 0.4rem;
    border-radius: 0.75rem;
    font-weight: 500;
    letter-spacing: 0.02em;
    font-size: var(--et-fs-meta);
}

.sort-dropdown {
    min-width: 6rem;
    max-width: 9rem;
}

.sort-direction-btn {
    font-size: 1rem;
    width: 2.5rem !important;
    height: 2.5rem !important;
}

@media (prefers-color-scheme: dark) {
    :deep(.device-card) {
        background-color: var(--surface-card, #1e293b);
        border-color: var(--surface-border, #334155);
        box-shadow: 0 1px 3px 0 rgba(0, 0, 0, 0.3);
    }

    :deep(.card-header) {
        color: var(--text-color, #f1f5f9);
    }

    .card-title {
        color: var(--text-color, #f1f5f9);
    }

    .card-subtitle {
        color: var(--text-color-secondary, #cbd5e1);
    }

    .version-badge {
        background-color: var(--primary-color, #0ea5e9);
    }

    :deep(.card-details) {
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
    border: 1px solid var(--surface-border);
    /* 不用 transition: all —— 只过渡实际会变的属性（悬停改边框色、聚焦加 ring） */
    transition: border-color 0.2s ease, box-shadow 0.2s ease, background-color 0.2s ease;
}

@media (hover: hover) {
    :deep(.p-dropdown:hover) {
        border-color: var(--primary-color);
    }
}

:deep(.p-button.p-button-icon-only.sort-direction-btn) {
    width: var(--et-btn);
    height: var(--et-btn);
}

.device-card-actions {
    flex-shrink: 0;
    align-items: center;
}

/* 数量徽章与两个操作按钮：同尺寸、同圆形、同悬停 */
.device-count-badge,
.device-card-actions :deep(.device-action-btn.p-button) {
    width: var(--et-btn-sm) !important;
    height: var(--et-btn-sm) !important;
    min-width: var(--et-btn-sm) !important;
    padding: 0 !important;
    box-sizing: border-box;
    border-radius: 9999px !important;
    border: 1px solid transparent !important;
    display: inline-flex !important;
    align-items: center;
    justify-content: center;
    line-height: 1;
    font-size: var(--et-fs-meta);
    font-weight: 600;
    cursor: default;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease, box-shadow 0.15s ease;
    box-shadow: none !important;
}

.device-card-actions :deep(.device-action-btn.p-button) {
    cursor: pointer;
}

.device-card-actions :deep(.device-action-btn .p-button-icon) {
    font-size: 0.875rem;
    line-height: 1;
    margin: 0 !important;
}

/* 默认态：统一浅底 */
.device-count-badge {
    background: var(--blue-50, #eff6ff) !important;
    color: var(--blue-700, #1d4ed8) !important;
}

.device-card-actions :deep(.device-action-btn.p-button-info),
.device-card-actions :deep(.device-action-btn.p-button-info .p-button-icon) {
    background: var(--blue-50, #eff6ff) !important;
    color: var(--blue-700, #1d4ed8) !important;
    border-color: transparent !important;
}

.device-card-actions :deep(.device-action-btn.p-button-secondary),
.device-card-actions :deep(.device-action-btn.p-button-secondary .p-button-icon) {
    background: var(--surface-100, #f3f4f6) !important;
    color: var(--text-color-secondary, #4b5563) !important;
    border-color: transparent !important;
}

/* 悬停态：统一加深底色，无位移、无额外阴影跳动。
   @media (hover: hover) 避免触屏点完「粘住」；:not(:disabled) 让禁用按钮不响应悬停。 */
@media (hover: hover) {
    .device-count-badge:hover,
    .device-card-actions :deep(.device-action-btn.p-button:hover:not(:disabled)),
    .device-card-actions :deep(.device-action-btn.p-button:hover:not(:disabled) .p-button-icon) {
        background: var(--surface-200, #e5e7eb) !important;
        color: var(--text-color, #1f2937) !important;
    }
}

.device-card-actions :deep(.device-action-btn.p-button:focus-visible),
.device-count-badge:focus-visible {
    outline: 2px solid var(--primary-color, #3b82f6);
    outline-offset: 2px;
}

.device-card-actions :deep(.device-action-btn.p-button:active) {
    background: var(--surface-300, #d1d5db) !important;
}

@media (prefers-color-scheme: dark) {
    .device-count-badge {
        background: rgba(59, 130, 246, 0.18) !important;
        color: var(--blue-300, #93c5fd) !important;
    }

    .device-card-actions :deep(.device-action-btn.p-button-info),
    .device-card-actions :deep(.device-action-btn.p-button-info .p-button-icon) {
        background: rgba(59, 130, 246, 0.18) !important;
        color: var(--blue-300, #93c5fd) !important;
    }

    .device-card-actions :deep(.device-action-btn.p-button-secondary),
    .device-card-actions :deep(.device-action-btn.p-button-secondary .p-button-icon) {
        background: var(--surface-hover, rgba(255, 255, 255, 0.08)) !important;
        color: var(--text-color-secondary, #cbd5e1) !important;
    }

    @media (hover: hover) {
        .device-count-badge:hover,
        .device-card-actions :deep(.device-action-btn.p-button:hover:not(:disabled)),
        .device-card-actions :deep(.device-action-btn.p-button:hover:not(:disabled) .p-button-icon) {
            background: var(--surface-hover, rgba(255, 255, 255, 0.14)) !important;
            color: var(--text-color, #f1f5f9) !important;
        }
    }
}

.location-icon {
    color: var(--pink-500);
    font-size: 0.9rem;
}

.location-text {
    font-size: 0.875rem;
    line-height: 1.25rem;
    opacity: 0.9;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
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
        color: var(--pink-400);
    }
}
.device-list-toolbar {
    background: var(--surface-card, #ffffff) !important;
    border: var(--et-border) !important;
    border-radius: var(--et-radius) !important;
    box-shadow: var(--et-shadow-card);
}

@media (max-width: 540px) {
    .device-list-toolbar {
        align-items: stretch;
    }

    .device-list-toolbar :deep(.p-toolbar-start),
    .device-list-toolbar :deep(.p-toolbar-end) {
        width: 100%;
    }

    .device-list-toolbar :deep(.p-toolbar-end) {
        justify-content: flex-start;
    }
}
</style>

<template>
    <div class="et-page">
        <div class="et-page-header">
            <h1 class="et-page-title">{{ t('web.device.list') }}</h1>
        </div>

        <Toolbar class="device-list-toolbar p-3 gap-4 surface-0 border-1 surface-border rounded-md">
            <template #start>
                <div class="flex items-center gap-2">
                    <label for="sort-by" class="text-sm text-500 hidden sm:block">{{ t('web.device.sort_by') }}：</label>
                    <Dropdown id="sort-by" v-model="selectedSortOption" :options="sortOptions" optionLabel="name"
                        class="sort-dropdown text-sm !min-w-[120px] sm:!min-w-[140px]" panelClass="text-sm">
                        <template #value="slotProps">
                            <div class="flex items-center gap-2">
                                <i :class="[slotProps.value.icon, 'text-600']"></i>
                                <span class="text-600">{{ slotProps.value.name() }}</span>
                            </div>
                        </template>
                        <template #option="slotProps">
                            <div class="flex items-center gap-2">
                                <i :class="[slotProps.option.icon, 'text-600']"></i>
                                <span>{{ slotProps.option.name() }}</span>
                            </div>
                        </template>
                    </Dropdown>
                    <Button :icon="ascending ? 'pi pi-sort-amount-up' : 'pi pi-sort-amount-down'" severity="secondary"
                        text rounded class="sort-direction-btn min-w-[2.5rem] h-[2.5rem]"
                        v-tooltip.top="ascending ? t('web.device.sort_direction_asc') : t('web.device.sort_direction_desc')"
                        @click="toggleSortDirection" />
                </div>
            </template>
            <template #end>
                    <div class="flex items-center gap-2">
                        <label for="detailed-view" class="text-sm text-500 hidden sm:block">{{
                            t('web.device.show_detailed_view') }}</label>
                        <InputSwitch id="detailed-view" v-model="showDetailedView" />
                    </div>
            </template>
        </Toolbar>

        <div v-if="deviceList === undefined" class="w-full flex justify-center">
            <ProgressSpinner />
        </div>

        <div v-else class="card-container">
                <div v-for="device in sortedDeviceList" :key="device.machine_id" class="device-card">
                    <div class="card-header">
                        <div class="flex justify-between items-center mb-2">
                            <div class="font-semibold truncate card-title" :title="device.hostname">{{ device.hostname
                            }}
                            </div>

                            <div class="text-xs version-badge" v-tooltip="`EasyTier ${device.easytier_version}`">
                                v{{ device.easytier_version.split('-')[0] }}
                            </div>
                        </div>

                        <div class="flex justify-between items-center">
                            <div class="text-sm truncate card-subtitle max-w-[60%] flex items-center gap-2"
                                :title="locationText(device)">
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
                                <!-- 运行中虚拟网数量（仅展示，悬停样式与按钮一致） -->
                                <span
                                    class="device-count-badge"
                                    :title="t('web.device.network_count')"
                                    :aria-label="`${t('web.device.network_count')}: ${device.running_network_count}`">
                                    {{ device.running_network_count }}
                                </span>

                                <Button v-tooltip.top="{ value: t('web.device.open_network_status') }"
                                    icon="pi pi-chart-line" severity="info" rounded text
                                    class="device-action-btn"
                                    @click="handleDeviceManagement(device, 'status')"
                                    :aria-label="t('web.device.open_network_status')" />

                                <Button v-tooltip.top="{ value: t('web.device.open_network_config') }"
                                    icon="pi pi-cog" severity="secondary" rounded text
                                    class="device-action-btn"
                                    @click="handleDeviceManagement(device, 'config')"
                                    :aria-label="t('web.device.open_network_config')" />
                            </div>
                        </div>
                    </div>

                    <div v-if="showDetailedView" class="card-details border-t surface-border fade-in">
                        <DeviceDetails :device="device" containerClass="card-details-content" :compact="true" />
                    </div>
                </div>
        </div>
    </div>
</template>
