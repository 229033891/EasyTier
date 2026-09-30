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
                    <Button icon="pi pi-trash" severity="danger" text rounded @click="removeUrl(index)" />
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
 * transition 只列真正变化的属性，不用 transition: all（会连带动画化无关属性，也影响性能）。
 */
.url-add-dropzone {
    transition: border-color 0.2s ease, background-color 0.2s ease;
}

@media (hover: hover) {
    .url-add-dropzone:hover {
        border-color: var(--primary-color, var(--et-primary, #0ea5e9));
        background-color: var(--surface-50, #f8fafc);
    }
}

@media (hover: hover) and (prefers-color-scheme: dark) {
    .url-add-dropzone:hover {
        background-color: var(--surface-800, #1e293b);
    }
}
</style>
