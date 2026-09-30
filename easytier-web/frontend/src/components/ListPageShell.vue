<script setup lang="ts">
import { ProgressSpinner } from 'primevue';

/**
 * 列表页脚手架：统一 loading / 空态 / 表格容器样式。
 *
 * 用法：默认插槽放表格；`empty` 插槽放空态文案。
 * 当 loading 为 true 时只显示 spinner，其余内容不渲染。
 */
defineProps<{
    /** true 表示数据尚未加载完成 */
    loading: boolean;
    /** true 表示加载完成但列表为空 */
    empty: boolean;
}>();
</script>

<template>
    <div v-if="loading" class="w-full flex justify-center py-8">
        <ProgressSpinner />
    </div>

    <div v-else-if="empty" class="et-list-empty et-meta py-10 px-4">
        <i class="pi pi-inbox text-2xl" aria-hidden="true"></i>
        <span><slot name="empty" /></span>
    </div>

    <div v-else class="et-list-table overflow-x-auto">
        <table class="w-full">
            <slot />
        </table>
    </div>
</template>

<style scoped>
.et-list-table,
.et-list-empty {
    overflow: hidden;
    background: var(--surface-card, #ffffff);
    border: var(--et-border);
    border-radius: var(--et-radius);
    box-shadow: var(--et-shadow-card, 0 1px 2px rgba(15, 23, 42, 0.03));
}

.et-list-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    min-height: 10rem;
    text-align: center;
}

.et-list-empty i {
    color: var(--primary-color, #0ea5e9);
    opacity: 0.72;
}

.et-list-table table {
    width: 100%;
    min-width: 0;
    border-collapse: collapse;
}

@media (min-width: 640px) {
    .et-list-table table {
        min-width: 38rem;
    }
}

.et-list-table th,
.et-list-table td {
    vertical-align: middle;
}

.et-list-table th {
    background: var(--surface-50, #f8fafc);
    color: var(--text-color-secondary, #64748b);
    font-size: var(--et-fs-meta);
    letter-spacing: 0.02em;
    text-transform: uppercase;
}

.et-list-table tbody tr {
    transition: background-color 0.15s ease;
}

@media (hover: hover) {
    .et-list-table tbody tr:hover {
        background: var(--surface-50, #f8fafc);
    }
}

@media (prefers-color-scheme: dark) {
    .et-list-table th {
        background: var(--surface-800, #1e293b);
    }

    @media (hover: hover) {
        .et-list-table tbody tr:hover {
            background: var(--surface-hover, rgba(255, 255, 255, 0.05));
        }
    }
}
</style>
