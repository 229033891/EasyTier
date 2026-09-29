import { createApp } from 'vue'
import 'easytier-frontend-lib/style.css'
import './style.css'
import App from './App.vue'
import EasytierFrontendLib from 'easytier-frontend-lib'
import PrimeVue from 'primevue/config'
import Aura from '@primeuix/themes/aura'
import { definePreset } from '@primeuix/themes'
import ConfirmationService from 'primevue/confirmationservice'
import { I18nUtils } from 'easytier-frontend-lib'

import { createRouter, createWebHashHistory } from 'vue-router'
import MainPage from './components/MainPage.vue'
import Login from './components/Login.vue'
import DeviceList from './components/DeviceList.vue'
import NetworkList from './components/NetworkList.vue'
import DeviceManagement from './components/DeviceManagement.vue'
import Dashboard from './components/Dashboard.vue'
import UserList from './components/UserList.vue'
import DialogService from 'primevue/dialogservice';
import ToastService from 'primevue/toastservice';
import { tooltipDirective } from './modules/tooltip'

const routes = [
    {
        path: '/auth', children: [
            {
                name: 'login',
                path: '',
                component: Login,
                alias: 'login',
            },
        ]
    },
    {
        path: '/h/:apiHost', component: MainPage, children: [
            {
                path: '',
                alias: 'dashboard',
                name: 'dashboard',
                component: Dashboard,
            },
            {
                path: 'deviceList',
                name: 'deviceList',
                component: DeviceList,
            },
            {
                path: 'networkList',
                name: 'networkList',
                component: NetworkList,
            },
            {
                path: 'userList',
                name: 'userList',
                component: UserList,
            },
            {
                // 独立全页管理（不再嵌在设备列表 Drawer 内）
                path: 'device/:deviceId/:instanceId?',
                name: 'deviceManagement',
                component: DeviceManagement,
            },
        ]
    },
    {
        path: '/:pathMatch(.*)*', name: 'notFound', redirect: () => {
            let apiHost = localStorage.getItem('apiHost');
            if (apiHost) {
                return { name: 'dashboard', params: { apiHost: apiHost } }
            } else {
                return { name: 'login' }
            }
        }
    }
]

const router = createRouter({
    history: createWebHashHistory(),
    routes,
})

/** 统一主色：与品牌蓝一致，避免各页绿/紫/蓝混用 */
const EasyTierPreset = definePreset(Aura, {
    semantic: {
        primary: {
            50: '{sky.50}',
            100: '{sky.100}',
            200: '{sky.200}',
            300: '{sky.300}',
            400: '{sky.400}',
            500: '{sky.500}',
            600: '{sky.600}',
            700: '{sky.700}',
            800: '{sky.800}',
            900: '{sky.900}',
            950: '{sky.950}',
        },
    },
})

const app = createApp(App)

// Use i18n
app.use(I18nUtils.i18n)

app.use(PrimeVue,
    {
        theme: {
            preset: EasyTierPreset,
            options: {
                prefix: 'p',
                darkModeSelector: 'system',
                cssLayer: {
                    name: 'primevue',
                    order: 'tailwind-base, primevue, tailwind-utilities'
                }
            }
        }
    }
)
app.use(ToastService as any)
app.use(DialogService as any)
app.use(router)
app.use(ConfirmationService as any)
app.use(EasytierFrontendLib)
app.directive('tooltip', tooltipDirective)
app.mount('#app')
