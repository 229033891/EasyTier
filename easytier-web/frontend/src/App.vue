<script setup lang="ts">
import { I18nUtils } from 'easytier-frontend-lib'
import { onMounted } from 'vue';
import { Toast, DynamicDialog, ConfirmDialog } from 'primevue';

onMounted(async () => {
  await I18nUtils.loadLanguageAsync(localStorage.getItem('lang') || 'en')
});

</script>

<!-- https://flowbite.com/docs/components/sidebar/#sidebar-with-navbar -->

<template>
  <Toast position="bottom-right" class="et-toast" />
  <ConfirmDialog class="et-confirm-dialog" />
  <DynamicDialog />
  <RouterView />
</template>
