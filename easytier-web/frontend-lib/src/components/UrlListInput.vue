<script setup lang="ts">
import { Button } from 'primevue'
import UrlInput from './UrlInput.vue'

const props = defineProps<{
    protos: { [proto: string]: number }
    addLabel: string
    placeholder?: string
    defaultUrl?: string
}>()

const list = defineModel<string[]>({ required: true })

const addUrl = () => {
    list.value.push(props.defaultUrl || 'tcp://0.0.0.0:11010')
}

const removeUrl = (index: number) => {
    list.value.splice(index, 1)
}
</script>

<template>
    <div class="flex flex-col gap-y-2 w-full">
        <div v-for="(_, index) in list" :key="index" class="flex gap-2 items-center w-full">
            <UrlInput v-model="list[index]" :protos="protos" :placeholder="placeholder">
                <template #actions>
                    <Button icon="pi pi-trash" severity="danger" text rounded class="et-icon-action-btn"
                        @click="removeUrl(index)" />
                </template>
            </UrlInput>
        </div>
        <div class="url-add-dropzone flex justify-center items-center w-full h-10 border-2 border-dashed border-surface-300 dark:border-surface-600 rounded-lg cursor-pointer gap-2 text-surface-500 dark:text-surface-400"
            @click="addUrl">
            <i class="pi pi-plus text-sm"></i>
            <span class="text-sm font-medium">{{ addLabel }}</span>
        </div>
    </div>
</template>

<style scoped>
/*
 * 悬停样式统一放进 @media (hover: hover)：
 * 触屏设备上 :hover 会「粘住」——点一下之后一直保持高亮，直到点别处，看起来像选中态。
 */
.url-add-dropzone {
    transition: border-color 0.2s ease, background-color 0.2s ease, color 0.2s ease;
    border-color: var(--et-border-color, #cbd5e1) !important;
    color: var(--text-color-secondary, #64748b) !important;
    border-radius: var(--et-radius, 0.75rem) !important;
}

@media (hover: hover) {
    .url-add-dropzone:hover {
        border-color: var(--primary-color, #0ea5e9) !important;
        background: color-mix(in srgb, var(--primary-color, #0ea5e9) 8%, #ffffff);
        color: var(--primary-color, #0ea5e9) !important;
    }
}

@media (hover: hover) and (prefers-color-scheme: dark) {
    .url-add-dropzone:hover {
        background: color-mix(in srgb, var(--primary-color, #0ea5e9) 14%, #0f172a);
        color: var(--primary-color, #0ea5e9) !important;
    }
}
</style>
