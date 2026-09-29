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

    <div v-else-if="empty" class="et-list-empty et-meta py-6 px-4">
        <slot name="empty" />
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
    background: var(--surface-ground, #f8fafc);
    border: var(--et-border);
    border-radius: var(--et-radius);
}

.et-list-table th,
.et-list-table td {
    vertical-align: middle;
}
</style>
