<script setup lang="ts">
import { tooltipDirective } from 'easytier-frontend-lib'
import { computed, onMounted, ref, onUnmounted, nextTick, watch } from 'vue';
import { Button } from 'primevue';
import { useRoute, useRouter } from 'vue-router';
import { useDialog } from 'primevue/usedialog';
import ChangePassword from './ChangePassword.vue';
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
    const menuWidth = Math.max(rect.width, 9.5 * 16);
    // 尽量居中对齐触发按钮，保证顶部小三角对准用户名
    let left = rect.left + rect.width / 2 - menuWidth / 2;
    left = Math.min(Math.max(8, left), window.innerWidth - menuWidth - 8);
    userMenuStyle.value = {
        position: 'fixed',
        top: `${Math.round(rect.bottom + 8)}px`,
        left: `${Math.round(left)}px`,
        width: `${Math.round(menuWidth)}px`,
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
        items.push({ name: 'configTokens', icon: 'pi pi-key', label: t('web.main.config_tokens') });
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
        if ((route.name === 'userList' || route.name === 'configTokens') && !isAdmin.value) {
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
                    :class="{ 'is-open': userMenuOpen }"
                    @click="toggleUserMenu">
                    <span class="sidebar-user-name truncate">{{ displayName }}</span>
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
        <!-- 顶部：桌面折叠 + 用户名；移动端抽屉内只显示品牌，用户名在顶栏 -->
        <div
            class="sidebar-brand flex shrink-0 items-center gap-1"
            :class="sidebarCollapsed ? 'justify-center px-1' : 'px-2'">
            <button type="button"
                class="sidebar-collapse-btn"
                :class="{ 'is-collapsed': sidebarCollapsed }"
                :aria-label="sidebarCollapsed ? t('web.main.expand_sidebar') : t('web.main.collapse_sidebar')"
                v-tooltip.right="sidebarCollapsed ? t('web.main.expand_sidebar') : t('web.main.collapse_sidebar')"
                @click="toggleDesktopCollapse">
                <span class="sidebar-collapse-icon" aria-hidden="true">
                    <span class="sidebar-collapse-bar"></span>
                    <span class="sidebar-collapse-bar sidebar-collapse-bar--mid"></span>
                    <span class="sidebar-collapse-bar"></span>
                </span>
            </button>
            <button ref="userTriggerRef" type="button" class="sidebar-user-trigger sidebar-user-trigger--desktop"
                :class="{
                    'sidebar-user-trigger--collapsed': sidebarCollapsed,
                    'is-open': userMenuOpen,
                }"
                :aria-expanded="userMenuOpen"
                aria-haspopup="menu"
                v-tooltip.right="sidebarCollapsed ? displayName : undefined"
                @click="toggleUserMenu">
                <span class="sidebar-user-name truncate">
                    {{ sidebarCollapsed ? displayName.slice(0, 1).toUpperCase() : displayName }}
                </span>
            </button>

            <div class="sidebar-brand-mobile">EasyTier</div>
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
            <button type="button" class="sidebar-user-menu-item" role="menuitem" @click="openChangePassword">
                <i class="pi pi-user" aria-hidden="true" />
                <span>{{ t('web.main.change_password') }}</span>
            </button>
            <div class="sidebar-user-menu-divider" role="separator"></div>
            <button type="button" class="sidebar-user-menu-item" role="menuitem"
                @click="doLogout">
                <i class="pi pi-power-off" aria-hidden="true" />
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
    padding-top: env(safe-area-inset-top);
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

.sidebar-brand-mobile {
    display: flex;
    align-items: center;
    min-width: 0;
    flex: 1 1 auto;
    padding: 0 0.55rem;
    font-size: 1rem;
    font-weight: 700;
    letter-spacing: -0.02em;
    color: var(--text-color, #1e293b);
}

.sidebar-user-trigger--desktop {
    display: none;
}

@media (min-width: 640px) {
    .sidebar-brand-mobile {
        display: none;
    }

    .sidebar-user-trigger--desktop {
        display: inline-flex;
    }
}

.sidebar-collapse-btn {
    display: none;
    align-items: center;
    justify-content: center;
    width: 2rem;
    height: 2rem;
    min-width: 2rem;
    padding: 0;
    border-radius: 999px;
    border: 1.5px solid color-mix(in srgb, var(--primary-color, #0ea5e9) 45%, #cbd5e1);
    background: color-mix(in srgb, var(--surface-0, #ffffff) 88%, var(--primary-color, #0ea5e9) 12%);
    color: var(--primary-color, #0284c7);
    cursor: pointer;
    box-shadow: 0 0 0 1px color-mix(in srgb, var(--primary-color, #0ea5e9) 10%, transparent);
    transition: transform 0.15s ease, box-shadow 0.15s ease, border-color 0.15s ease, background 0.15s ease, color 0.15s ease;
}

@media (min-width: 640px) {
    .sidebar-collapse-btn {
        display: inline-flex;
    }
}

.sidebar-collapse-icon {
    position: relative;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 0.18rem;
    width: 0.85rem;
    height: 0.72rem;
    transition: transform 0.2s ease;
}

/* 展开态：箭头朝左表示可折叠；折叠态保持朝右表示可展开 */
.sidebar-collapse-btn:not(.is-collapsed) .sidebar-collapse-icon {
    transform: scaleX(-1);
}

.sidebar-collapse-bar {
    display: block;
    width: 100%;
    height: 1.5px;
    border-radius: 999px;
    background: currentColor;
}

.sidebar-collapse-bar--mid {
    position: relative;
    width: 62%;
}

.sidebar-collapse-bar--mid::after {
    content: "";
    position: absolute;
    right: -0.28rem;
    top: 50%;
    width: 0;
    height: 0;
    border-style: solid;
    border-width: 0.22rem 0 0.22rem 0.28rem;
    border-color: transparent transparent transparent currentColor;
    transform: translateY(-50%);
}

@media (hover: hover) {
    .sidebar-collapse-btn:hover {
        background: color-mix(in srgb, var(--surface-0, #ffffff) 72%, var(--primary-color, #0ea5e9) 28%);
        border-color: var(--primary-color, #0ea5e9);
        color: color-mix(in srgb, var(--primary-color, #0284c7) 85%, #0f172a);
        box-shadow: 0 0 0 2px color-mix(in srgb, var(--primary-color, #0ea5e9) 22%, transparent);
        transform: scale(1.04);
    }
}

.sidebar-collapse-btn:active {
    transform: scale(0.96);
}

.sidebar-user-trigger {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.35rem;
    min-width: 0;
    flex: 1 1 auto;
    max-width: 100%;
    height: 2rem;
    padding: 0 0.9rem;
    border: none;
    border-radius: 999px;
    background: var(--primary-color, var(--et-primary, #0ea5e9));
    color: #ffffff;
    font-size: 0.875rem;
    font-weight: 700;
    letter-spacing: -0.01em;
    cursor: pointer;
    text-align: center;
    box-shadow: 0 1px 2px color-mix(in srgb, var(--primary-color, #0ea5e9) 28%, transparent);
    transition: background-color 0.15s ease, box-shadow 0.15s ease, transform 0.15s ease;
}

.sidebar-user-trigger--mobile {
    flex: 0 1 auto;
    max-width: 60%;
}

.sidebar-user-trigger--collapsed {
    flex: 0 0 auto;
    width: 2rem;
    height: 2rem;
    padding: 0;
}

@media (hover: hover) {
    .sidebar-user-trigger:hover {
        background: var(--primary-600, var(--et-primary-emphasis, #0284c7));
        box-shadow: 0 2px 8px color-mix(in srgb, var(--primary-color, #0ea5e9) 35%, transparent);
    }
}

.sidebar-user-trigger.is-open {
    background: var(--primary-600, var(--et-primary-emphasis, #0284c7));
}

.sidebar-user-trigger:active {
    transform: scale(0.98);
}

.sidebar-user-name {
    min-width: 0;
    width: 100%;
    color: inherit;
    text-align: center;
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
        padding-left: max(0.75rem, env(safe-area-inset-left));
        padding-right: max(0.75rem, env(safe-area-inset-right));
        padding-bottom: max(0.5rem, env(safe-area-inset-bottom));
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
    position: relative;
    min-width: 9.5rem;
    padding: 0.4rem 0.35rem;
    margin-top: 0.35rem;
    border: none;
    border-radius: 0.65rem;
    background: #2f3542;
    box-shadow: 0 10px 28px rgba(15, 23, 42, 0.28);
    color: #ffffff;
}

.sidebar-user-menu::before {
    content: "";
    position: absolute;
    top: -0.35rem;
    left: 50%;
    width: 0;
    height: 0;
    border-style: solid;
    border-width: 0 0.4rem 0.4rem 0.4rem;
    border-color: transparent transparent #2f3542 transparent;
    transform: translateX(-50%);
}

.sidebar-user-menu-divider {
    height: 1px;
    margin: 0.2rem 0.55rem;
    background: rgba(255, 255, 255, 0.12);
}

.sidebar-user-menu-item {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    width: 100%;
    min-height: 2.15rem;
    padding: 0 0.7rem;
    border: none;
    border-radius: 0.4rem;
    background: transparent;
    color: #ffffff;
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    text-align: left;
}

.sidebar-user-menu-item i {
    width: 1rem;
    font-size: 0.9rem;
    text-align: center;
    opacity: 0.92;
}

.sidebar-user-menu-item:hover {
    background: rgba(255, 255, 255, 0.08);
}
</style>
