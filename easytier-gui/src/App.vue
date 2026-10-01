<script setup lang="ts">
import { getCurrentWindow } from '@tauri-apps/api/window'
import { ConfirmDialog } from 'primevue'
import pkg from '~/../package.json'

onBeforeMount(async () => {
  await getCurrentWindow().setTitle(`ET: v${pkg.version}`)
})
</script>

<template>
  <Toast position="bottom-right" />
  <ConfirmDialog class="et-confirm-dialog" />
  <RouterView />
</template>
