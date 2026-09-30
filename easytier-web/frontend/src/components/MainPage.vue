<script setup lang="ts">
import { I18nUtils, tooltipDirective } from 'easytier-frontend-lib'
import { computed, onMounted, ref, onUnmounted, nextTick, watch } from 'vue';
import { Button } from 'primevue';
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

/**
 * 账户操作直接作为侧栏条目，不再用 TieredMenu 弹层。
 *
 * 原因：PrimeVue 的 popup 菜单是按「页面坐标」定位的（targetRect.top + scrollTop），
 * 而侧栏是 position:fixed + translate-x-*（transform 会成为绝对/固定定位后代的包含块），
 * 弹层坐标会被当成相对侧栏解析，于是菜单跑到错位、看起来就像「不见了」。
 */
const openChangePassword = () => {
    dialog.open(ChangePassword, {
        props: {
            modal: true,
        },
        data: {
            api: api.value,
        }
    });
};

const doLogout = async () => {
    try {
        await api.value?.logout();
    } catch (e) {
        console.error("logout failed", e);
    }
    router.push({ name: 'login' });
};

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

/**
 * 折叠态：桌面居中图标；展开态：左对齐 + 间距。
 *
 * 移动端抽屉始终是展开样式（w-64），所以折叠分支也要先给 `justify-start`，
 * 再由 `sm:justify-center` 在 ≥640px 覆盖。
 */
const sidebarButtonClass = computed(() =>
    sidebarCollapsed.value
        ? 'justify-start gap-x-3 pl-1.5 sm:justify-center sm:gap-x-0 sm:pl-0'
        : 'justify-start gap-x-3 pl-1.5'
);

const navRef = ref<HTMLElement>();
let navResizeObserver: ResizeObserver | undefined;

/**
 * 顶栏（移动端专属，sticky 占据文档流）的实测高度写入 --et-navbar-h，
 * 供内容区高度与侧栏内边距使用，避免按固定值估算导致的双倍占位 / 底部溢出。
 *
 * 桌面端顶栏 display:none → offsetHeight 为 0，因此这里**必须允许写入 0px**，
 * 内容区才能用满 100dvh（顶栏高度不再占用纵向空间）。
 * 另外 display:none 的元素 ResizeObserver 不一定回调，所以始终额外监听 window.resize。
 */
const syncNavbarHeight = () => {
    const h = navRef.value?.offsetHeight ?? 0;
    document.documentElement.style.setProperty('--et-navbar-h', `${h}px`);
};

onMounted(async () => {
    await nextTick();
    document.addEventListener('click', handleClickOutside);
    syncNavbarHeight();
    if (typeof ResizeObserver !== 'undefined' && navRef.value) {
        navResizeObserver = new ResizeObserver(syncNavbarHeight);
        navResizeObserver.observe(navRef.value);
    }
    window.addEventListener('resize', syncNavbarHeight);
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
    <!-- 顶栏仅保留在移动端：只放抽屉开关 + 品牌，把纵向空间全部让给内容区 -->
    <nav ref="navRef"
        class="sm:hidden fixed top-0 z-50 w-full top-navbar et-shell-surface">
        <div class="px-3 py-2">
            <div class="flex items-center justify-start rtl:justify-end gap-1">
                <div ref="toggleButtonRef">
                    <Button type="button" aria-haspopup="true" icon="pi pi-bars"
                        variant="text" size="large" severity="contrast"
                        :aria-label="t('web.main.toggle_sidebar')"
                        v-tooltip.bottom="t('web.main.toggle_sidebar')"
                        @click="toggleMobileSidebar" />
                </div>
                <div class="flex ms-1 items-center">
                    <img :src="Icon" class="h-8 me-3" :alt="t('web.main.logo_alt')" />
                    <span
                        class="self-center text-xl font-semibold whitespace-nowrap et-shell-text">EasyTier</span>
                </div>
            </div>
        </div>
    </nav>

    <div v-if="forceShowSideBar" class="fixed inset-0 z-30 bg-black bg-opacity-50 sm:hidden" @click="closeSidebar">
    </div>

    <aside ref="sidebarRef" id="logo-sidebar"
        class="fixed top-0 left-0 z-40 flex h-screen flex-col et-shell-surface et-shell-border-r"
        :class="[
            forceShowSideBar ? 'translate-x-0' : '-translate-x-full',
            'sm:translate-x-0',
            sidebarCollapsed ? 'sm:w-16' : 'sm:w-64',
            'w-64',
        ]"
        :aria-label="t('web.main.sidebar')">
        <!-- 品牌区：移动端顶栏已有品牌，这里只在桌面显示（纯展示，无外链） -->
        <div
            class="sidebar-brand hidden sm:flex shrink-0 items-center et-shell-border-b"
            :class="sidebarCollapsed ? 'justify-center px-0' : 'px-3'">
            <img :src="Icon" class="h-8" :alt="t('web.main.logo_alt')" />
            <span class="sidebar-brand-text ms-3 text-xl font-semibold whitespace-nowrap et-shell-text"
                :class="{ 'sm:hidden': sidebarCollapsed }">EasyTier</span>
        </div>

        <div class="flex-1 min-h-0 overflow-y-auto px-2 py-3 et-shell-surface">
            <ul class="space-y-2 font-medium">
                <li v-for="item in navItems" :key="item.name">
                    <Button variant="text"
                        class="w-full sidebar-button"
                        :class="[sidebarButtonClass, { 'sidebar-button--active': route.name === item.name }]"
                        severity="contrast" @click="goNav(item.name)"
                        :aria-current="route.name === item.name ? 'page' : undefined"
                        v-tooltip.right="sidebarCollapsed ? item.label : undefined">
                        <i :class="[item.icon, 'text-xl']"></i>
                        <span class="mb-0.5" :class="{ 'sm:hidden': sidebarCollapsed }">{{ item.label }}</span>
                    </Button>
                </li>
            </ul>
        </div>

        <!-- 底部固定区：语言 / 修改密码 / 登出 / 折叠 -->
        <div class="shrink-0 et-shell-border-t px-2 py-2 et-shell-surface">
            <ul class="space-y-2 font-medium">
                <li>
                    <Button variant="text" class="w-full sidebar-button" :class="sidebarButtonClass"
                        severity="contrast" @click="I18nUtils.toggleLanguage"
                        :aria-label="t('web.main.language')"
                        v-tooltip.right="sidebarCollapsed ? t('web.main.language') : undefined">
                        <i class="pi pi-globe text-xl opacity-80"></i>
                        <span class="mb-0.5" :class="{ 'sm:hidden': sidebarCollapsed }">{{ t('web.main.language')
                        }}</span>
                    </Button>
                </li>
                <li>
                    <Button variant="text" class="w-full sidebar-button" :class="sidebarButtonClass"
                        severity="contrast" @click="openChangePassword"
                        :aria-label="t('web.main.change_password')"
                        v-tooltip.right="sidebarCollapsed ? t('web.main.change_password') : undefined">
                        <i class="pi pi-lock text-xl opacity-80"></i>
                        <span class="mb-0.5" :class="{ 'sm:hidden': sidebarCollapsed }">{{
                            t('web.main.change_password') }}</span>
                    </Button>
                </li>
                <li>
                    <Button variant="text" class="w-full sidebar-button sidebar-logout" :class="sidebarButtonClass"
                        severity="contrast" @click="doLogout" :aria-label="t('web.main.logout')"
                        v-tooltip.right="sidebarCollapsed ? t('web.main.logout') : undefined">
                        <i class="pi pi-sign-out text-xl opacity-80"></i>
                        <span class="mb-0.5" :class="{ 'sm:hidden': sidebarCollapsed }">{{ t('web.main.logout')
                        }}</span>
                    </Button>
                </li>
                <li class="hidden sm:block">
                    <Button variant="text" class="w-full sidebar-button" :class="sidebarButtonClass"
                        severity="contrast" @click="toggleDesktopCollapse"
                        :aria-label="sidebarCollapsed ? t('web.main.expand_sidebar') : t('web.main.collapse_sidebar')"
                        v-tooltip.right="sidebarCollapsed ? t('web.main.expand_sidebar') : t('web.main.collapse_sidebar')">
                        <i :class="[sidebarCollapsed ? 'pi pi-angle-double-right' : 'pi pi-angle-double-left', 'text-xl opacity-80']"></i>
                        <span class="mb-0.5" :class="{ 'sm:hidden': sidebarCollapsed }">
                            {{ sidebarCollapsed ? t('web.main.expand_sidebar') : t('web.main.collapse_sidebar') }}
                        </span>
                    </Button>
                </li>
            </ul>
        </div>
    </aside>

    <div class="et-main-content"
        :class="[sidebarCollapsed ? 'sm:ml-16' : 'sm:ml-64', { 'et-main-content--mgmt': isManagementPage }]">
        <RouterView v-slot="{ Component }">
            <div :class="{ 'et-main-panel': !isManagementPage }">
                <component :is="Component" :api="api" />
            </div>
        </RouterView>
    </div>
</template>

<style scoped>
/* 壳层（侧栏/顶栏）统一走 surface token，避免 Tailwind gray 与内容区脱节 */
.et-shell-surface {
    background: var(--surface-card, #ffffff);
}

.et-shell-text {
    color: var(--text-color, #1e293b);
}

.et-shell-border-r {
    border-right: 1px solid var(--surface-border, #e2e8f0);
}

.et-shell-border-b {
    border-bottom: 1px solid var(--surface-border, #e2e8f0);
}

.et-shell-border-t {
    border-top: 1px solid var(--surface-border, #e2e8f0);
}

.top-navbar.et-shell-surface {
    border-bottom: 1px solid var(--surface-border, #e2e8f0);
}

/* 侧栏条目：只保留文字对齐。
   不要在这里写 justify-content —— scoped 规则特异性 (0,2,0) 会压过
   Tailwind 的 sm:justify-center (0,1,0)，折叠态图标就没法居中了。
   对齐完全交给 sidebarButtonClass 里的工具类。 */
.sidebar-button {
    min-height: 2.75rem;
    border-radius: 0.625rem !important;
    color: var(--text-color-secondary, #64748b) !important;
    text-align: left;
    transition: background-color 0.18s ease, color 0.18s ease;
}

@media (hover: hover) {
    .sidebar-button:hover:not(:disabled) {
        background: var(--surface-hover, #f1f5f9) !important;
        color: var(--text-color, #1e293b) !important;
    }
}

.sidebar-button--active,
.sidebar-button--active:hover {
    background: color-mix(in srgb, var(--primary-color, var(--et-primary, #0ea5e9)) 11%, transparent) !important;
    color: var(--primary-color, var(--et-primary-emphasis, #0284c7)) !important;
    box-shadow: inset 3px 0 0 var(--primary-color, var(--et-primary, #0ea5e9));
}

.sidebar-button i {
    width: 1.25rem;
    line-height: 1;
    text-align: center;
}

/* 底栏图标统一降 20% 不透明度，和上方导航粗细拉齐；登出 hover 才显红，避免常驻大红 */
.sidebar-logout:hover,
.sidebar-logout:hover i {
    color: var(--p-red-500, #ef4444) !important;
}

/* 侧栏顶部品牌条：桌面端顶栏已移除，这里承担品牌展示 */
.sidebar-brand {
    height: 3.5rem;
}

/* 侧栏为 fixed，需让位给顶栏：用实测顶栏高度对齐其底边。
   桌面端顶栏 display:none，--et-navbar-h 为 0，侧栏直接从顶部开始。
   过渡只列实际会变的属性（折叠改 width、抽屉改 transform），不用 transition: all。 */
#logo-sidebar {
    padding-top: var(--et-navbar-h, 0px);
    transition: width 0.2s ease, transform 0.2s ease;
}

.et-main-content {
    min-height: 100dvh;
    box-sizing: border-box;
    padding: 0.75rem 1rem 1.5rem;
    background: var(--surface-ground, #f6f8fb);
    /* 顶栏使用 fixed，移动端必须显式让出实测高度，避免首屏内容被盖住 */
    padding-top: calc(var(--et-navbar-h, 0px) + max(0.75rem, env(safe-area-inset-top, 0px)));
    transition: margin-left 0.2s ease;
}

.et-main-content--mgmt {
    box-sizing: border-box;
    /* 扣掉顶栏高度（桌面端为 0），否则底部溢出（按钮栏被挤出视口） */
    height: calc(100dvh - var(--et-navbar-h, 0px));
    max-height: calc(100dvh - var(--et-navbar-h, 0px));
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    /* 高度已经扣除 fixed 顶栏，不能再次把顶栏高度塞进内部 padding */
    padding-top: max(0.25rem, env(safe-area-inset-top, 0px));
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
    width: 100%;
    max-width: 1440px;
    margin: 0 auto;
    padding: 0.75rem 0.5rem 0;
}

@media (max-width: 640px) {
    .et-main-content {
        padding-left: 0.75rem;
        padding-right: 0.75rem;
    }

    .et-main-panel {
        padding-left: 0;
        padding-right: 0;
    }
}
</style>
