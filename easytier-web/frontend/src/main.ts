import { createApp } from 'vue'
import 'easytier-frontend-lib/style.css'
import './style.css'
import App from './App.vue'
import EasytierFrontendLib from 'easytier-frontend-lib'
import ConfirmationService from 'primevue/confirmationservice'
import { createRouter, createWebHashHistory } from 'vue-router'
import MainPage from './components/MainPage.vue'
import Login from './components/Login.vue'
import DeviceList from './components/DeviceList.vue'
import NetworkList from './components/NetworkList.vue'
import DeviceManagement from './components/DeviceManagement.vue'
import Dashboard from './components/Dashboard.vue'
import UserList from './components/UserList.vue'
import DialogService from 'primevue/dialogservice'
import ToastService from 'primevue/toastservice'

const routes = [
    {
        path: '/auth', children: [
            {
                name: 'login',
                path: '',
                component: Login,
                alias: 'login',
            },
            {
                name: 'register',
                path: 'register',
                redirect: { name: 'login' },
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

const app = createApp(App)

// 主题 / PrimeVue / tooltip / 共享组件 / i18n 由 frontend-lib 统一安装
app.use(EasytierFrontendLib)
app.use(ToastService as any)
app.use(DialogService as any)
app.use(ConfirmationService as any)
app.use(router)
app.mount('#app')
