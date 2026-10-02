import { createApp } from 'vue'
import EasytierFrontendLib from 'easytier-frontend-lib'
import { ConfirmationService, ToastService } from 'primevue'
import 'easytier-frontend-lib/style.css'
import App from './App.vue'

// PrimeVue + EasyTierPreset 由 frontend-lib 统一安装；Toast/Confirm 与 Web/GUI 一致
createApp(App)
  .use(EasytierFrontendLib)
  .use(ToastService)
  .use(ConfirmationService)
  .mount('#app')
