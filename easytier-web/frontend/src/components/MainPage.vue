<script setup lang="ts">
import { I18nUtils } from 'easytier-frontend-lib'
import { computed, onMounted, ref, onUnmounted, nextTick, watch } from 'vue';
import { Button, TieredMenu } from 'primevue';
import { tooltipDirective } from '../modules/tooltip';
import { useRoute, useRouter } from 'vue-router';
import { useDialog } from 'primevue/usedialog';
import ChangePassword from './ChangePassword.vue';
import Icon from '../assets/easytier.png'
import { useI18n } from 'vue-i18n'
import ApiClient from '../modules/api';

const vTooltip = tooltipDirective;

const { t } = useI18n()
const route = useRoute();
const router = useRouter();
const api = computed<ApiClient | undefined>(() => {
    try {
        return new ApiClient(atob(route.params.apiHost as string), () => {
            router.push({ name: 'login' });
        })
    } catch (e) {
        router.push({ name: 'login' });
    }
});

const isAdmin = ref(false);

const dialog = useDialog();

const userMenu = ref();
const userMenuItems = computed(() => [
    {
        label: t('web.main.change_password'),
        icon: 'pi pi-key',
        command: () => {
            dialog.open(ChangePassword, {
                props: {
                    modal: true,
                },
                data: {
                    api: api.value,
                }
            });
        },
    },
    {
        label: t('web.main.logout'),
        icon: 'pi pi-sign-out',
        command: async () => {
            try {
                await api.value?.logout();
            } catch (e) {
                console.error("logout failed", e);
            }
            router.push({ name: 'login' });
        },
    },
])

/** 移动端：抽屉显隐 */
const forceShowSideBar = ref(false)
/** 桌面端：折叠为图标栏 */
const sidebarCollapsed = ref(localStorage.getItem('easytier-web.sidebarCollapsed') === 'true')
watch(sidebarCollapsed, (v) => {
    localStorage.setItem('easytier-web.sidebarCollapsed', String(v));
});

const sidebarRef = ref<HTMLElement>()
const toggleButtonRef = ref<HTMLElement>()

const handleClickOutside = (event: Event) => {
    const target = event.target as HTMLElement;
    if (!forceShowSideBar.value) return;
    const isClickInsideSidebar = sidebarRef.value?.contains(target);
    const isClickOnToggleButton = toggleButtonRef.value?.contains(target);
    if (!isClickInsideSidebar && !isClickOnToggleButton) {
        forceShowSideBar.value = false;
    }
};

const toggleMobileSidebar = () => {
    forceShowSideBar.value = !forceShowSideBar.value;
};

const toggleDesktopCollapse = () => {
    sidebarCollapsed.value = !sidebarCollapsed.value;
};

const closeSidebar = () => {
    forceShowSideBar.value = false;
};

const goNav = (name: string) => {
    router.push({ name });
    forceShowSideBar.value = false;
};

const isManagementPage = computed(() => route.name === 'deviceManagement');

/** 侧栏导航项：dashboard / deviceList / networkList 常显，userList 仅管理员 */
const navItems = computed(() => {
    const items = [
        { name: 'dashboard', icon: 'pi pi-chart-pie', label: t('web.main.dashboard') },
        { name: 'deviceList', icon: 'pi pi-server', label: t('web.main.device_list') },
        { name: 'networkList', icon: 'pi pi-sitemap', label: t('web.main.network_list') },
    ];
    if (isAdmin.value) {
        items.push({ name: 'userList', icon: 'pi pi-users', label: t('web.main.user_list') });
    }
    return items;
});

/** 折叠态：图标居中；展开态：左对齐 + 间距 */
const sidebarButtonClass = computed(() =>
    sidebarCollapsed.value ? 'sm:justify-center sm:pl-0' : 'justify-start gap-x-3 pl-1.5'
);

/**
 * 顶栏是 sticky（.top-navbar 覆盖了 Tailwind 的 .fixed），会占据文档流。
 * 把实测高度写入 --et-navbar-h，供内容区高度与侧栏内边距使用，
 * 避免按 fixed 顶栏估算导致的双倍占位 / 底部溢出。
 */
const navRef = ref<HTMLElement>();
let navResizeObserver: ResizeObserver | undefined;

const syncNavbarHeight = () => {
    const h = navRef.value?.offsetHeight;
    if (h) {
        document.documentElement.style.setProperty('--et-navbar-h', `${h}px`);
    }
};

onMounted(async () => {
    await nextTick();
    document.addEventListener('click', handleClickOutside);
    syncNavbarHeight();
    if (typeof ResizeObserver !== 'undefined' && navRef.value) {
        navResizeObserver = new ResizeObserver(syncNavbarHeight);
        navResizeObserver.observe(navRef.value);
    } else {
        window.addEventListener('resize', syncNavbarHeight);
    }
    try {
        const me = await api.value?.get_me();
        isAdmin.value = !!me?.is_admin;
        if (route.name === 'userList' && !isAdmin.value) {
            router.replace({ name: 'dashboard' });
        }
    } catch (e) {
        console.error('load me failed', e);
        isAdmin.value = false;
    }
});

onUnmounted(() => {
    document.removeEventListener('click', handleClickOutside);
    navResizeObserver?.disconnect();
    window.removeEventListener('resize', syncNavbarHeight);
});

</script>

<template>
    <nav ref="navRef"
        class="fixed top-0 z-50 w-full bg-white border-b border-gray-200 dark:bg-gray-800 dark:border-gray-700 top-navbar">
        <div class="px-3 py-2 lg:px-5 lg:pl-3">
            <div class="flex items-center justify-between">
                <div class="flex items-center justify-start rtl:justify-end gap-1">
                    <!-- 移动端：打开/关闭抽屉 -->
                    <div class="sm:hidden" ref="toggleButtonRef">
                        <Button type="button" aria-haspopup="true" icon="pi pi-bars"
                            variant="text" size="large" severity="contrast"
                            :aria-label="t('web.main.toggle_sidebar')"
                            v-tooltip.bottom="t('web.main.toggle_sidebar')"
                            @click="toggleMobileSidebar" />
                    </div>
                    <!-- 桌面端：折叠/展开侧栏 -->
                    <div class="hidden sm:block">
                        <Button type="button"
                            :icon="sidebarCollapsed ? 'pi pi-angle-right' : 'pi pi-angle-left'"
                            variant="text" size="large" severity="contrast"
                            :aria-label="sidebarCollapsed ? t('web.main.expand_sidebar') : t('web.main.collapse_sidebar')"
                            v-tooltip.bottom="sidebarCollapsed ? t('web.main.expand_sidebar') : t('web.main.collapse_sidebar')"
                            @click="toggleDesktopCollapse" />
                    </div>
                    <a href="https://easytier.top" class="flex ms-1 md:me-24">
                        <img :src="Icon" class="h-8 me-3" :alt="t('web.main.logo_alt')" />
                        <span
                            class="self-center text-xl font-semibold sm:text-2xl whitespace-nowrap dark:text-white">EasyTier</span>
                    </a>
                </div>
                <div class="flex items-center gap-3">
                    <Button icon="pi pi-language" @click="I18nUtils.toggleLanguage" rounded severity="contrast"
                        :aria-label="t('web.main.language')"
                        v-tooltip.bottom="t('web.main.language')" />
                    <Button type="button" @click="userMenu.toggle($event)" aria-haspopup="true"
                        aria-controls="user-menu" icon="pi pi-user" raised rounded
                        :aria-label="t('web.main.user_menu')" />
                    <TieredMenu ref="userMenu" id="user-menu" :model="userMenuItems" popup />
                </div>
            </div>
        </div>
    </nav>

    <div v-if="forceShowSideBar" class="fixed inset-0 z-30 bg-black bg-opacity-50 sm:hidden" @click="closeSidebar">
    </div>

    <aside ref="sidebarRef" id="logo-sidebar"
            class="fixed top-0 left-0 z-40 h-screen transition-all duration-200 bg-white border-r border-gray-200 dark:bg-gray-800 dark:border-gray-700"
        :class="[
            forceShowSideBar ? 'translate-x-0' : '-translate-x-full',
            'sm:translate-x-0',
            sidebarCollapsed ? 'sm:w-16' : 'sm:w-64',
            'w-64',
        ]"
        :aria-label="t('web.main.sidebar')">
        <div class="h-full px-2 pb-4 overflow-y-auto bg-white dark:bg-gray-800">
            <ul class="space-y-2 font-medium">
                <li v-for="item in navItems" :key="item.name">
                    <Button variant="text"
                        class="w-full sidebar-button"
                        :class="sidebarButtonClass"
                        severity="contrast" @click="goNav(item.name)"
                        v-tooltip.right="sidebarCollapsed ? item.label : undefined">
                        <i :class="[item.icon, 'text-xl']"></i>
                        <span class="mb-0.5" :class="{ 'sm:hidden': sidebarCollapsed }">{{ item.label }}</span>
                    </Button>
                </li>
            </ul>
        </div>
    </aside>

    <div class="et-main-content transition-all duration-200"
        :class="[sidebarCollapsed ? 'sm:ml-16' : 'sm:ml-64', { 'et-main-content--mgmt': isManagementPage }]">
        <RouterView v-slot="{ Component }">
            <div :class="{ 'et-main-panel': !isManagementPage }">
                <component :is="Component" :api="api" />
            </div>
        </RouterView>
    </div>
</template>

<style scoped>
.sidebar-button {
    text-align: left;
    justify-content: left;
}

/* 侧栏为 fixed，需让位给顶栏：用实测顶栏高度对齐其底边 */
#logo-sidebar {
    padding-top: var(--et-navbar-h, 3.5rem);
}

.et-main-content {
    padding: 0 0.75rem 0.75rem;
    /* 顶栏是 sticky（.top-navbar），已占据文档流，无需再预留高度 */
    padding-top: env(safe-area-inset-top, 0px);
}

.et-main-content--mgmt {
    box-sizing: border-box;
    /* 顶栏为 sticky 且占据文档流，需扣掉其高度，否则底部溢出（按钮栏被挤出视口） */
    height: calc(100dvh - var(--et-navbar-h, 3.0625rem));
    max-height: calc(100dvh - var(--et-navbar-h, 3.0625rem));
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    padding-top: env(safe-area-inset-top, 0px);
    padding-bottom: 0.25rem;
}

/* 管理页：外层仅作透传容器，内层路由组件占满高度 */
.et-main-content--mgmt > * {
    flex: 1 1 auto;
    min-height: 0;
    height: 100%;
    overflow: hidden;
    display: flex;
    flex-direction: column;
}

.et-main-content--mgmt > * > * {
    flex: 1 1 auto;
    min-height: 0;
}

.et-main-panel {
    background: var(--surface-card, #ffffff);
    border: var(--et-border);
    border-radius: var(--et-radius);
    padding: 0.5rem 0.75rem 0.75rem;
}

@media (prefers-color-scheme: dark) {
    .et-main-panel {
        background: var(--surface-card, #1e293b);
    }
}
</style>
