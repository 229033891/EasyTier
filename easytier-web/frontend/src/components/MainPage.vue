<script setup lang="ts">
import { I18nUtils, tooltipDirective } from 'easytier-frontend-lib'
import { computed, onMounted, ref, onUnmounted, nextTick, watch } from 'vue';
import { Button } from 'primevue';
import { useRoute, useRouter } from 'vue-router';
import { useDialog } from 'primevue/usedialog';
import ChangePassword from './ChangePassword.vue';
import { useI18n } from 'vue-i18n'
import ApiClient from '../modules/api';

const vTooltip = tooltipDirective;

const { t, locale } = useI18n()
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
const username = ref('');

const dialog = useDialog();

const openChangePassword = () => {
    closeUserMenu();
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
    closeUserMenu();
    try {
        await api.value?.logout();
    } catch (e) {
        console.error("logout failed", e);
    }
    router.push({ name: 'login' });
};

const setLanguage = async (lang: 'cn' | 'en') => {
    await I18nUtils.loadLanguageAsync(lang);
    closeUserMenu();
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
const userTriggerRef = ref<HTMLElement>()
const mobileUserTriggerRef = ref<HTMLElement>()
const userMenuOpen = ref(false)
const userMenuStyle = ref<Record<string, string>>({})

const activeUserTrigger = () => {
    if (typeof window !== 'undefined' && window.innerWidth < 640)
        return mobileUserTriggerRef.value || userTriggerRef.value
    return userTriggerRef.value || mobileUserTriggerRef.value
}

const handleClickOutside = (event: Event) => {
    const target = event.target as HTMLElement;
    if (userMenuOpen.value) {
        const inDesktop = userTriggerRef.value?.contains(target);
        const inMobile = mobileUserTriggerRef.value?.contains(target);
        const inMenu = target.closest?.('.sidebar-user-menu');
        if (!inDesktop && !inMobile && !inMenu) {
            closeUserMenu();
        }
    }
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
    closeUserMenu();
    sidebarCollapsed.value = !sidebarCollapsed.value;
};

const closeSidebar = () => {
    forceShowSideBar.value = false;
};

const closeUserMenu = () => {
    userMenuOpen.value = false;
};

const syncUserMenuPosition = () => {
    const el = activeUserTrigger();
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const menuWidth = 12.5 * 16; // ~12.5rem
    let left = rect.left;
    if (left + menuWidth > window.innerWidth - 8) {
        left = Math.max(8, window.innerWidth - menuWidth - 8);
    }
    userMenuStyle.value = {
        position: 'fixed',
        top: `${Math.round(rect.bottom + 6)}px`,
        left: `${Math.round(left)}px`,
        minWidth: `${Math.max(rect.width, 180)}px`,
        zIndex: '1200',
    };
};

const toggleUserMenu = async () => {
    if (userMenuOpen.value) {
        closeUserMenu();
        return;
    }
    userMenuOpen.value = true;
    await nextTick();
    syncUserMenuPosition();
};

const goNav = (name: string) => {
    router.push({ name });
    forceShowSideBar.value = false;
    closeUserMenu();
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
 * 管理全页不属于侧栏路由名：按来源高亮设备/网络列表。
 * query.from=networkList → 网络列表；其余（含缺省）→ 设备列表。
 */
const isNavActive = (name: string) => {
    if (route.name === name) return true;
    if (route.name !== 'deviceManagement') return false;
    if (name === 'networkList') return route.query.from === 'networkList';
    if (name === 'deviceList') return route.query.from !== 'networkList';
    return false;
};

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

const displayName = computed(() => username.value || t('web.users.username'));
const currentLang = computed(() => (locale.value === 'cn' ? 'cn' : 'en'));

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
    window.addEventListener('resize', syncNavbarHeight);
    window.addEventListener('resize', syncUserMenuPosition);
    window.addEventListener('scroll', syncUserMenuPosition, true);
    syncNavbarHeight();
    if (typeof ResizeObserver !== 'undefined' && navRef.value) {
        navResizeObserver = new ResizeObserver(syncNavbarHeight);
        navResizeObserver.observe(navRef.value);
    }
    try {
        const me = await api.value?.get_me();
        isAdmin.value = !!me?.is_admin;
        username.value = me?.username || '';
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
    window.removeEventListener('resize', syncUserMenuPosition);
    window.removeEventListener('scroll', syncUserMenuPosition, true);
});

</script>

<template>
    <!-- 顶栏仅保留在移动端：抽屉开关 + 用户名菜单 -->
    <nav ref="navRef"
        class="sm:hidden fixed top-0 z-50 w-full top-navbar et-shell-surface">
        <div class="px-3 py-2">
            <div class="flex items-center justify-between gap-2">
                <div ref="toggleButtonRef">
                    <Button type="button" aria-haspopup="true" icon="pi pi-bars"
                        variant="text" size="large" severity="contrast"
                        :aria-label="t('web.main.toggle_sidebar')"
                        v-tooltip.bottom="t('web.main.toggle_sidebar')"
                        @click="toggleMobileSidebar" />
                </div>
                <button ref="mobileUserTriggerRef" type="button" class="sidebar-user-trigger sidebar-user-trigger--mobile"
                    @click="toggleUserMenu">
                    <span class="sidebar-user-name truncate">{{ displayName }}</span>
                    <i class="pi pi-angle-down text-sm opacity-70" aria-hidden="true" />
                </button>
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
            sidebarCollapsed ? 'sm:w-16 sidebar--collapsed' : 'sm:w-64',
            'w-64',
        ]"
        :aria-label="t('web.main.sidebar')">
        <!-- 顶部：折叠 + 用户名菜单（移动端抽屉内也显示） -->
        <div
            class="sidebar-brand flex shrink-0 items-center gap-1"
            :class="sidebarCollapsed ? 'justify-center px-1' : 'px-2'">
            <Button type="button" variant="text" severity="contrast"
                class="sidebar-collapse-btn hidden sm:inline-flex"
                :icon="sidebarCollapsed ? 'pi pi-angle-double-right' : 'pi pi-angle-double-left'"
                :aria-label="sidebarCollapsed ? t('web.main.expand_sidebar') : t('web.main.collapse_sidebar')"
                v-tooltip.right="sidebarCollapsed ? t('web.main.expand_sidebar') : t('web.main.collapse_sidebar')"
                @click="toggleDesktopCollapse" />

            <button ref="userTriggerRef" type="button" class="sidebar-user-trigger"
                :class="{ 'sidebar-user-trigger--collapsed': sidebarCollapsed }"
                :aria-expanded="userMenuOpen"
                aria-haspopup="menu"
                v-tooltip.right="sidebarCollapsed ? displayName : undefined"
                @click="toggleUserMenu">
                <i v-if="sidebarCollapsed" class="pi pi-user sidebar-icon" aria-hidden="true" />
                <template v-else>
                    <span class="sidebar-user-name truncate">{{ displayName }}</span>
                    <i class="pi pi-angle-down text-sm opacity-70 shrink-0" aria-hidden="true" />
                </template>
            </button>
        </div>

        <div class="sidebar-nav flex-1 min-h-0 overflow-y-auto px-2 py-2">
            <ul class="sidebar-nav-list">
                <li v-for="item in navItems" :key="item.name">
                    <Button variant="text"
                        class="w-full sidebar-button"
                        :class="[sidebarButtonClass, { 'sidebar-button--active': isNavActive(item.name) }]"
                        severity="contrast" @click="goNav(item.name)"
                        :aria-current="isNavActive(item.name) ? 'page' : undefined"
                        v-tooltip.right="sidebarCollapsed ? item.label : undefined">
                        <i :class="[item.icon, 'sidebar-icon']"></i>
                        <span :class="{ 'sm:hidden': sidebarCollapsed }">{{ item.label }}</span>
                    </Button>
                </li>
            </ul>
        </div>
    </aside>

    <!-- 账户菜单挂到 body，避免侧栏 transform 导致定位错乱 -->
    <Teleport to="body">
        <div v-if="userMenuOpen" class="sidebar-user-menu" role="menu" :style="userMenuStyle">
            <div class="sidebar-user-menu-section">
                <div class="sidebar-user-menu-label">{{ t('web.main.language') }}</div>
                <div class="sidebar-user-lang">
                    <button type="button" class="sidebar-user-lang-btn"
                        :class="{ 'is-active': currentLang === 'cn' }"
                        @click="setLanguage('cn')">中文</button>
                    <button type="button" class="sidebar-user-lang-btn"
                        :class="{ 'is-active': currentLang === 'en' }"
                        @click="setLanguage('en')">English</button>
                </div>
            </div>
            <button type="button" class="sidebar-user-menu-item" role="menuitem" @click="openChangePassword">
                <i class="pi pi-lock" aria-hidden="true" />
                <span>{{ t('web.main.change_password') }}</span>
            </button>
            <button type="button" class="sidebar-user-menu-item sidebar-user-menu-item--danger" role="menuitem"
                @click="doLogout">
                <i class="pi pi-sign-out" aria-hidden="true" />
                <span>{{ t('web.main.logout') }}</span>
            </button>
        </div>
    </Teleport>

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
/* 壳层：侧栏略偏冷灰，与内容区白卡片拉开层次 */
.et-shell-surface {
    background: var(--surface-50, #f8fafc);
}

.et-shell-text {
    color: var(--text-color, #1e293b);
}

.et-shell-border-r {
    border-right: 1px solid var(--et-border-color, var(--surface-border, #e2e8f0));
    box-shadow: 1px 0 0 rgba(15, 23, 42, 0.02);
}

.top-navbar.et-shell-surface {
    background: var(--surface-card, #ffffff);
    border-bottom: 1px solid var(--et-border-color, var(--surface-border, #e2e8f0));
}

.sidebar-nav-list {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    margin: 0;
    padding: 0;
    list-style: none;
    font-weight: 500;
}

/* 侧栏条目：只保留文字对齐。
   不要在这里写 justify-content —— scoped 规则特异性 (0,2,0) 会压过
   Tailwind 的 sm:justify-center (0,1,0)，折叠态图标就没法居中了。
   对齐完全交给 sidebarButtonClass 里的工具类。 */
.sidebar-button {
    position: relative;
    min-height: 2.5rem;
    border-radius: calc(var(--et-radius, 0.75rem) - 0.125rem) !important;
    color: var(--text-color-secondary, #64748b) !important;
    text-align: left;
    font-size: var(--et-fs-body, 0.875rem);
    font-weight: 500;
    transition: background-color 0.18s ease, color 0.18s ease;
}

.sidebar-icon {
    width: 1.25rem;
    font-size: 1.05rem;
    line-height: 1;
    text-align: center;
    color: inherit;
    opacity: 0.92;
}

@media (hover: hover) {
    .sidebar-button:hover:not(:disabled) {
        background: color-mix(in srgb, var(--text-color, #1e293b) 5%, transparent) !important;
        color: var(--text-color, #1e293b) !important;
    }
}

.sidebar-button--active,
.sidebar-button--active:hover {
    background: color-mix(in srgb, var(--primary-color, var(--et-primary, #0ea5e9)) 14%, transparent) !important;
    color: var(--primary-color, var(--et-primary-emphasis, #0284c7)) !important;
    font-weight: 600;
}

.sidebar-button--active .sidebar-icon {
    opacity: 1;
}

/* 选中指示条：圆角胶囊贴在按钮内侧，避免 inset shadow 与圆角打架 */
.sidebar-button--active::before {
    content: "";
    position: absolute;
    left: 0.2rem;
    top: 0.55rem;
    bottom: 0.55rem;
    width: 0.2rem;
    border-radius: 999px;
    background: var(--primary-color, var(--et-primary, #0ea5e9));
}

/* 折叠态图标居中，指示条会挡图标，桌面折叠时隐藏 */
@media (min-width: 640px) {
    .sidebar--collapsed .sidebar-button--active::before {
        display: none;
    }
}

.sidebar-brand {
    height: 3.5rem;
    border-bottom: 1px solid var(--et-border-color, var(--surface-border, #e2e8f0));
}

.sidebar-collapse-btn {
    width: 2.25rem !important;
    height: 2.25rem !important;
    min-width: 2.25rem !important;
    padding: 0 !important;
    color: var(--text-color-secondary, #64748b) !important;
}

.sidebar-user-trigger {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
    flex: 1 1 auto;
    height: 2.25rem;
    padding: 0 0.55rem;
    border: none;
    border-radius: calc(var(--et-radius, 0.75rem) - 0.25rem);
    background: transparent;
    color: var(--text-color, #1e293b);
    font-size: 0.95rem;
    font-weight: 700;
    letter-spacing: -0.02em;
    cursor: pointer;
    text-align: left;
}

.sidebar-user-trigger--mobile {
    flex: 0 1 auto;
    max-width: 60%;
}

.sidebar-user-trigger--collapsed {
    flex: 0 0 auto;
    width: 2.25rem;
    justify-content: center;
    padding: 0;
}

@media (hover: hover) {
    .sidebar-user-trigger:hover {
        background: color-mix(in srgb, var(--text-color, #1e293b) 5%, transparent);
    }
}

.sidebar-user-name {
    min-width: 0;
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

<!-- 账户菜单 teleport 到 body，需非 scoped 才能稳定命中 -->
<style>
.sidebar-user-menu {
    padding: 0.4rem;
    border: 1px solid var(--et-border-color, #e2e8f0);
    border-radius: var(--et-radius, 0.75rem);
    background: var(--surface-card, #ffffff);
    box-shadow: 0 12px 28px rgba(15, 23, 42, 0.14);
}

.sidebar-user-menu-section {
    padding: 0.45rem 0.55rem 0.55rem;
    margin-bottom: 0.25rem;
    border-bottom: 1px solid var(--et-border-color, #e2e8f0);
}

.sidebar-user-menu-label {
    margin-bottom: 0.35rem;
    color: var(--text-color-secondary, #64748b);
    font-size: var(--et-fs-meta, 0.75rem);
    font-weight: 600;
}

.sidebar-user-lang {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.35rem;
}

.sidebar-user-lang-btn {
    height: 2rem;
    border: 1px solid var(--et-border-color, #e2e8f0);
    border-radius: calc(var(--et-radius, 0.75rem) - 0.35rem);
    background: var(--surface-50, #f8fafc);
    color: var(--text-color, #1e293b);
    font-size: 0.8125rem;
    font-weight: 600;
    cursor: pointer;
}

.sidebar-user-lang-btn.is-active {
    border-color: color-mix(in srgb, var(--primary-color, #0ea5e9) 45%, var(--et-border-color, #e2e8f0));
    background: color-mix(in srgb, var(--primary-color, #0ea5e9) 12%, transparent);
    color: var(--primary-color, #0284c7);
}

.sidebar-user-menu-item {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    width: 100%;
    min-height: 2.35rem;
    padding: 0 0.65rem;
    border: none;
    border-radius: calc(var(--et-radius, 0.75rem) - 0.35rem);
    background: transparent;
    color: var(--text-color, #1e293b);
    font-size: 0.875rem;
    font-weight: 500;
    cursor: pointer;
    text-align: left;
}

.sidebar-user-menu-item:hover {
    background: color-mix(in srgb, var(--text-color, #1e293b) 5%, transparent);
}

.sidebar-user-menu-item--danger {
    color: var(--et-danger, #ef4444);
}

.sidebar-user-menu-item--danger:hover {
    background: color-mix(in srgb, var(--et-danger, #ef4444) 8%, transparent);
}
</style>
