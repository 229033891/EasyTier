<script setup lang="ts">
/**
 * 表单字段容器：label + 控件。
 * 不预设 label 样式，由调用方通过 label-class 控制，避免影响既有观感。
 * 外部传入的 class 会合并到根元素，便于复用调用方自身的 .field / .field-admin 等布局类。
 */
defineProps<{
    label?: string;
    /** label 的 for，对应控件 id；纯展示型字段可不传 */
    labelFor?: string;
    labelClass?: string;
}>();
</script>

<template>
    <div class="form-field">
        <label v-if="label" :for="labelFor" :class="labelClass">{{ label }}</label>
        <slot />
    </div>
</template>

<style scoped>
.form-field {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
}
</style>
