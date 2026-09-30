// 占位文件，勿在此写死 api_host。
//
// 运行时优先级：
//   1. easytier-web 启动时带 --api-host 时，后端会以 /api_meta.js 覆盖本文件（见 src/web/mod.rs）；
//   2. 否则前端回退到当前页面地址 location.origin（见 src/modules/api-host.ts）。
//
// 因此这里保持空对象，登录页的「API 主机」就会默认显示当前访问的 URL。
window.apiMeta = {}
