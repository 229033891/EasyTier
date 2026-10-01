<script setup lang="ts">
import { getEasytierVersion } from '~/composables/backend'

const { t } = useI18n()

const etVersion = ref('')

onMounted(async () => {
  etVersion.value = await getEasytierVersion()
})
</script>

<template>
  <div class="flex flex-col gap-3">
    <div class="flex flex-col gap-2">
      <label>{{ t('about.version') }}</label>
      <InputText :model-value="etVersion || '...'" class="w-full" readonly />
    </div>
    <p class="text-sm text-secondary m-0 leading-relaxed whitespace-pre-line">
      {{ t('about.description') }}
    </p>
  </div>
</template>

<style scoped lang="postcss">
</style>
