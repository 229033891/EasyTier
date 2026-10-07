import EasyTierFrontendLib, { I18nUtils } from 'easytier-frontend-lib'
import { ConfirmationService, DialogService, ToastService } from 'primevue'
import { createRouter, createWebHistory } from 'vue-router/auto'
// `vue-router/auto-routes` is a virtual module (unplugin-vue-router) that only
// happens to resolve to the same file as `vue-router/auto` for ESLint's node
// resolver. They must stay two separate imports — see the eslint.config.js
// override for this file.
import { routes } from 'vue-router/auto-routes'

import App from '~/App.vue'
import 'easytier-frontend-lib/style.css'
import '~/styles.css'

if (import.meta.env.PROD) {
  document.addEventListener('keydown', (event) => {
    if (
      event.key === 'F5'
      || (event.ctrlKey && event.key === 'r')
      || (event.metaKey && event.key === 'r')
    ) {
      event.preventDefault()
    }
  })

  document.addEventListener('contextmenu', (event) => {
    event.preventDefault()
  })
}

async function main() {
  await I18nUtils.loadLanguageAsync(localStorage.getItem('lang') || 'en')

  const app = createApp(App)

  const router = createRouter({
    history: createWebHistory(),
    routes,
  })

  app.use(router)
  app.use(createPinia())
  // 主题 / PrimeVue / tooltip 由 frontend-lib 统一安装（EasyTierPreset）
  app.use(EasyTierFrontendLib)
  app.use(ToastService)
  app.use(DialogService)
  app.use(ConfirmationService)
  app.mount('#app')
}

main()
