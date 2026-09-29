interface ApiHost {
    value: string;
    usedAt: number;
}

let apiMeta: {
    api_host: string;
} | undefined = (window as any).apiMeta;

// remove trailing slashes from the URL
const cleanUrl = (url: string) => url.replace(/\/+$/, '');

/** 当前页面地址（一体包部署时通常就是 API 地址） */
const currentPageApiHost = cleanUrl(location.origin);

/** 启动参数 --api-host 注入值，其次当前访问 URL */
const defaultApiHost = cleanUrl(apiMeta?.api_host || currentPageApiHost);

const isValidHttpUrl = (s: string): boolean => {
    let url;

    try {
        url = new URL(s);
    } catch (_) {
        return false;
    }

    return url.protocol === "http:" || url.protocol === "https:";
};

const cleanAndLoadApiHosts = (): Array<ApiHost> => {
    const maxHosts = 10;
    const apiHosts = localStorage.getItem('apiHosts');
    if (apiHosts) {
        const hosts: Array<ApiHost> = JSON.parse(apiHosts);
        // sort by usedAt
        hosts.sort((a, b) => b.usedAt - a.usedAt);

        // only keep the first 10
        if (hosts.length > maxHosts) {
            hosts.splice(maxHosts);
        }

        localStorage.setItem('apiHosts', JSON.stringify(hosts));
        return hosts;
    } else {
        return [];
    }
};

const saveApiHost = (host: string) => {
    console.log('Save API Host:', host);
    if (!isValidHttpUrl(host)) {
        console.error('Invalid API Host:', host);
        return;
    }

    let hosts = cleanAndLoadApiHosts();
    const newHost: ApiHost = { value: host, usedAt: Date.now() };
    hosts = hosts.filter((h) => h.value !== host);
    hosts.push(newHost);
    localStorage.setItem('apiHosts', JSON.stringify(hosts));
};

/**
 * 登录页默认显示：
 * 1. 服务端注入的 --api-host（若有）
 * 2. 否则当前浏览器访问地址（location.origin）
 * 历史主机仍可通过下拉选择。
 */
const getInitialApiHost = (): string => {
    return defaultApiHost;
};

export { getInitialApiHost, cleanAndLoadApiHosts, saveApiHost }