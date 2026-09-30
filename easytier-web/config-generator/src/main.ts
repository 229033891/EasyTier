import { createApp } from 'vue'
import EasytierFrontendLib from 'easytier-frontend-lib'
import 'easytier-frontend-lib/style.css'
import App from './App.vue'

// PrimeVue + EasyTierPreset 由 frontend-lib 统一安装
createApp(App).use(EasytierFrontendLib).mount('#app')
