import axios, { AxiosError, AxiosInstance, AxiosResponse, InternalAxiosRequestConfig } from 'axios';
import { type Api, NetworkTypes, Utils, normalizeLoggerLevel, loggerLevelToRpc } from 'easytier-frontend-lib';

export interface ValidateConfigResponse {
    toml_config: string;
}

export interface OidcConfigResponse {
    enabled: boolean;
}

// 定义接口返回的数据结构
export interface LoginResponse {
    success: boolean;
    message: string;
}

export interface MeResponse {
    id: number;
    username: string;
    is_admin: boolean;
    config_token?: string;
    config_tokens?: string[];
}

export interface UserInfo {
    id: number;
    username: string;
    groups: string[];
    is_admin: boolean;
    config_token?: string;
    config_tokens?: string[];
}

export interface ConfigTokenInfo {
    id: number;
    user_id: number;
    username: string;
    token: string;
    label: string;
    /** RFC3339 with offset — format with Utils.formatEventTime for display. */
    create_time: string;
    update_time: string;
}

export interface CreateConfigTokenRequest {
    user_id: number;
    token?: string;
    label?: string;
}

export interface UpdateConfigTokenRequest {
    token?: string;
    label?: string;
}

export interface AdminCreateUserRequest {
    username: string;
    password: string;
    is_admin?: boolean;
}

export interface Credential {
    username: string;
    password: string;
}

export interface Summary {
    device_count: number;
    network_count: number;
}

export interface DiagnosticDetail {
    code: string;
    param?: string | null;
}

export interface DiagnosticCheck {
    id: string;
    status: string;
    expected: string;
    actual: string;
    detail?: DiagnosticDetail | null;
}

export interface OverallSummary {
    code: string;
    params: string[];
}

export interface DiagnosticsSnapshot {
    version: string;
    api_listen: string;
    config_server_port: number;
    config_server_protocol: string;
    config_server_listening: string[];
    heartbeat_min_response_ms: number;
    heartbeat_timeout_ms: number;
    db_path: string;
    console_log_level?: string | null;
    file_log_dir?: string | null;
    webhook_configured: boolean;
    oidc_configured: boolean;
}

export interface SystemDiagnosticsResponse {
    overall: string;
    summary: OverallSummary;
    generated_at: string;
    checks: DiagnosticCheck[];
    snapshot: DiagnosticsSnapshot;
}

export interface RuntimeLogLine {
    ts: string;
    level: string;
    target: string;
    message: string;
}

export interface RuntimeLogsResponse {
    capacity: number;
    lines: RuntimeLogLine[];
}

export interface LogFileInfo {
    file_name: string;
    size_bytes: number;
    modified_ms: number;
    active: boolean;
}

export interface LogFilesResponse {
    enabled: boolean;
    dir: string;
    level: string;
    files: LogFileInfo[];
}

export interface LogFileQueryParams {
    file: string;
    tail?: number;
    min_level?: string;
    since?: string;
    until?: string;
    grep?: string;
}

export interface LogFileLinesResponse {
    file: string;
    dir: string;
    level: string;
    total_scanned: number;
    matched: number;
    truncated_bytes: boolean;
    lines: RuntimeLogLine[];
}

export interface ListNetworkInstanceIdResponse {
    running_inst_ids: Array<Utils.UUID>,
    disabled_inst_ids: Array<Utils.UUID>,
}

export interface GenerateConfigRequest {
    config: NetworkTypes.NetworkConfig;
}

export interface GenerateConfigResponse {
    toml_config?: string;
    error?: string;
}

export interface ParseConfigRequest {
    toml_config: string;
}

export interface ParseConfigResponse {
    config?: NetworkTypes.NetworkConfig;
    error?: string;
}

export class ApiClient {
    private client: AxiosInstance;
    private authFailedCb: Function | undefined;

    constructor(baseUrl: string, authFailedCb: Function | undefined = undefined) {
        this.client = axios.create({
            baseURL: baseUrl.replace(/\/+$/, '') + '/api/v1',
            withCredentials: true, // 如果需要支持跨域携带cookie
            headers: {
                'Content-Type': 'application/json',
            },
        });
        this.authFailedCb = authFailedCb;

        // 添加请求拦截器
        this.client.interceptors.request.use((config: InternalAxiosRequestConfig) => {
            return config;
        }, (error: any) => {
            return Promise.reject(error);
        });

        // 添加响应拦截器
        this.client.interceptors.response.use((response: AxiosResponse) => {
            console.debug('Axios Response:', response);
            return response.data; // 假设服务器返回的数据都在data属性中
        }, (error: any) => {
            if (error.response) {
                let response: AxiosResponse = error.response;
                if (response.status == 401 && this.authFailedCb) {
                    console.error('Unauthorized:', response.data);
                    this.authFailedCb();
                } else {
                    // 请求已发出，但是服务器响应的状态码不在2xx范围
                    console.error('Response Error:', error.response.data);
                }
            } else if (error.request) {
                // 请求已发出，但是没有收到响应
                console.error('Request Error:', error.request);
            } else {
                // 发生了一些问题导致请求未发出
                console.error('Error:', error.message);
            }
            return Promise.reject(error);
        });
    }

    public async get_me(): Promise<MeResponse> {
        return await this.client.get<any, MeResponse>('/auth/me');
    }

    public async list_users(): Promise<UserInfo[]> {
        return await this.client.get<any, UserInfo[]>('/users');
    }

    public async create_user(data: AdminCreateUserRequest): Promise<UserInfo> {
        const payload = {
            username: data.username,
            password: data.password,
            is_admin: !!data.is_admin,
        };
        return await this.client.post<any, UserInfo>('/users', payload);
    }

    public async delete_user(id: number): Promise<void> {
        await this.client.delete(`/users/${id}`);
    }

    public async reset_user_password(id: number, new_password: string): Promise<void> {
        await this.client.put(`/users/${id}/password`, { new_password });
    }

    public async get_system_diagnostics(): Promise<SystemDiagnosticsResponse> {
        return await this.client.get<any, SystemDiagnosticsResponse>('/admin/system-diagnostics');
    }

    public async get_runtime_logs(tail: number = 500): Promise<RuntimeLogsResponse> {
        return await this.client.get<any, RuntimeLogsResponse>('/admin/logs', {
            params: { tail },
        });
    }

    public async list_log_files(): Promise<LogFilesResponse> {
        return await this.client.get<any, LogFilesResponse>('/admin/logs/files');
    }

    public async read_log_file(params: LogFileQueryParams): Promise<LogFileLinesResponse> {
        return await this.client.get<any, LogFileLinesResponse>('/admin/logs/file', {
            params,
        });
    }

    public async list_config_tokens(): Promise<ConfigTokenInfo[]> {
        return await this.client.get<any, ConfigTokenInfo[]>('/admin/config-tokens');
    }

    public async create_config_token(data: CreateConfigTokenRequest): Promise<ConfigTokenInfo> {
        return await this.client.post<any, ConfigTokenInfo>('/admin/config-tokens', data);
    }

    public async update_config_token(id: number, data: UpdateConfigTokenRequest): Promise<ConfigTokenInfo> {
        return await this.client.put<any, ConfigTokenInfo>(`/admin/config-tokens/${id}`, data);
    }

    public async delete_config_token(id: number): Promise<void> {
        await this.client.delete(`/admin/config-tokens/${id}`);
    }

    // 登录
    public async login(data: Credential): Promise<LoginResponse> {
        try {
            const response = await this.client.post<any>('/auth/login', data);
            console.log("login response:", response);
            return { success: true, message: 'Login success', };
        } catch (error) {
            if (error instanceof AxiosError && error.response?.status === 401) {
                return { success: false, message: 'invalid_credentials' };
            }
            const detail = Utils.extractApiErrorPayload(error).message;
            return {
                success: false,
                message: detail || 'unknown_error',
            };
        }
    }

    public async logout() {
        await this.client.get('/auth/logout');
        if (this.authFailedCb) {
            this.authFailedCb();
        }
    }

    public async change_password(new_password: string) {
        await this.client.put('/auth/password', { new_password });
    }

    public async check_login_status() {
        try {
            await this.client.get('/auth/check_login_status');
            return true;
        } catch (error) {
            return false;
        }
    }

    public async list_session() {
        const response = await this.client.get('/sessions');
        return response;
    }

    public async list_machines(): Promise<Array<any>> {
        const response = await this.client.get<any, Record<string, Array<any>>>('/machines');
        return response.machines;
    }

    public async list_devices(): Promise<Array<{
        device_id: string;
        hostname: string;
        display_name?: string;
        last_easytier_version: string;
        last_client_url: string;
        last_seen_at: number;
    }>> {
        const response = await this.client.get<any, { devices: Array<any> }>('/devices');
        return response.devices || [];
    }

    public async update_device_display_name(deviceId: string, displayName: string): Promise<void> {
        await this.client.put(`/devices/${deviceId}`, {
            display_name: displayName,
        });
    }

    /** Retire an offline device from the archive (and drop its stored configs). */
    public async delete_device(deviceId: string): Promise<void> {
        await this.client.delete(`/devices/${deviceId}`);
    }

    /**
     * Merge source archive identity into target (configs + alias + history),
     * then remove the source device row.
     */
    public async merge_devices(sourceDeviceId: string, targetDeviceId: string): Promise<void> {
        await this.client.post('/devices/merge', {
            source_device_id: sourceDeviceId,
            target_device_id: targetDeviceId,
        });
    }

    public async get_summary(): Promise<Summary> {
        const response = await this.client.get<any, Summary>('/summary');
        return response;
    }

    public async getOidcConfig(): Promise<OidcConfigResponse> {
        try {
            const response = await this.client.get<any, OidcConfigResponse>('/auth/oidc/config');
            return response;
        } catch (error) {
            // Older servers may not expose the optional endpoint; other failures
            // should remain visible instead of being mistaken for "SSO disabled".
            if (error instanceof AxiosError && error.response?.status === 404) {
                return { enabled: false };
            }
            throw error;
        }
    }

    public oidcLoginUrl() {
        return this.client.defaults.baseURL + '/auth/oidc/login';
    }

    public get_remote_client(machine_id: string): Api.RemoteClient {
        return new WebRemoteClient(machine_id, this.client);
    }
}

class WebRemoteClient implements Api.RemoteClient {
    private machine_id: string;
    private client: AxiosInstance;

    constructor(machine_id: string, client: AxiosInstance) {
        this.machine_id = machine_id;
        this.client = client;
    }
    async validate_config(config: NetworkTypes.NetworkConfig): Promise<Api.ValidateConfigResponse> {
        const response = await this.client.post<NetworkTypes.NetworkConfig, ValidateConfigResponse>(`/machines/${this.machine_id}/validate-config`, {
            config: NetworkTypes.toBackendNetworkConfig(config),
        });
        return response;
    }
    async run_network(config: NetworkTypes.NetworkConfig, save: boolean): Promise<undefined> {
        await this.client.post<string>(`/machines/${this.machine_id}/networks`, {
            config: NetworkTypes.toBackendNetworkConfig(config),
            save: save
        });
    }
    async get_network_info(inst_id: string): Promise<NetworkTypes.NetworkInstanceRunningInfo | undefined> {
        const response = await this.client.get<any, Api.CollectNetworkInfoResponse>('/machines/' + this.machine_id + '/networks/info/' + inst_id);
        return response.info?.map?.[inst_id];
    }
    async get_network_infos(inst_ids: string[]): Promise<Record<string, NetworkTypes.NetworkInstanceRunningInfo | undefined>> {
        if (!inst_ids.length) {
            return {};
        }
        const response = await this.client.post<any, Api.CollectNetworkInfoResponse>(
            `/machines/${this.machine_id}/networks/info`,
            { inst_ids },
        );
        return response.info?.map ?? {};
    }
    async get_vpn_portal_info(inst_id: string): Promise<NetworkTypes.VpnPortalInfo | undefined> {
        const response = await this.client.post<any, { vpn_portal_info?: NetworkTypes.VpnPortalInfo }>(
            `/machines/${this.machine_id}/proxy-rpc`,
            {
                service_name: 'api.instance.VpnPortalRpcService',
                method_name: 'get_vpn_portal_info',
                payload: {
                    instance: {
                        id: Utils.StrToUuid(inst_id),
                    },
                },
            },
        );
        return response.vpn_portal_info
            ? NetworkTypes.normalizeVpnPortalInfo(response.vpn_portal_info)
            : undefined;
    }
    async patch_vpn_portal_clients(inst_id: string, patches: Array<Record<string, any>>): Promise<undefined> {
        await this.client.post(
            `/machines/${this.machine_id}/proxy-rpc`,
            {
                service_name: 'api.config.ConfigRpcService',
                method_name: 'patch_config',
                payload: {
                    instance: {
                        id: Utils.StrToUuid(inst_id),
                    },
                    patch: {
                        vpn_portal_clients: patches,
                    },
                },
            },
        );
    }
    async add_vpn_portal_client(inst_id: string, client: { name: string, virtual_ip: string, groups: string[] }): Promise<undefined> {
        await this.patch_vpn_portal_clients(inst_id, [{
            action: 'ADD',
            client,
        }]);
    }
    async remove_vpn_portal_client(inst_id: string, name: string): Promise<undefined> {
        await this.patch_vpn_portal_clients(inst_id, [{
            action: 'REMOVE',
            client: { name, virtual_ip: '', groups: [] },
        }]);
    }
    async clear_vpn_portal_clients(inst_id: string): Promise<undefined> {
        await this.patch_vpn_portal_clients(inst_id, [{ action: 'CLEAR' }]);
    }
    async list_network_instance_ids(): Promise<Api.ListNetworkInstanceIdResponse> {
        const response = await this.client.get<any, ListNetworkInstanceIdResponse>('/machines/' + this.machine_id + '/networks');
        return response;
    }
    async delete_network(inst_id: string): Promise<undefined> {
        await this.client.delete<string>(`/machines/${this.machine_id}/networks/${inst_id}`);
    }
    async update_network_instance_state(inst_id: string, disabled: boolean): Promise<undefined> {
        await this.client.put<string>('/machines/' + this.machine_id + '/networks/' + inst_id, {
            disabled: disabled,
        });
    }
    async save_config(config: NetworkTypes.NetworkConfig): Promise<undefined> {
        await this.client.put(`/machines/${this.machine_id}/networks/config/${config.instance_id}`, {
            config: NetworkTypes.toBackendNetworkConfig(config)
        });
    }
    async get_network_config(inst_id: string): Promise<NetworkTypes.NetworkConfig> {
        const response = await this.client.get<any, NetworkTypes.NetworkConfig>('/machines/' + this.machine_id + '/networks/config/' + inst_id);
        return NetworkTypes.normalizeNetworkConfig(response);
    }
    async generate_config(config: NetworkTypes.NetworkConfig): Promise<Api.GenerateConfigResponse> {
        try {
            const response = await this.client.post<any, GenerateConfigResponse>('/generate-config', {
                config: NetworkTypes.toBackendNetworkConfig(config)
            });
            return response;
        } catch (error) {
            return { error: Utils.extractApiErrorPayload(error).message || 'Unknown error' };
        }
    }
    async parse_config(toml_config: string): Promise<Api.ParseConfigResponse> {
        try {
            const response = await this.client.post<any, ParseConfigResponse>('/parse-config', { toml_config });
            if (response.config) {
                response.config = NetworkTypes.normalizeNetworkConfig(response.config);
            }
            return response;
        } catch (error) {
            return { error: Utils.extractApiErrorPayload(error).message || 'Unknown error' };
        }
    }
    async get_network_metas(instance_ids: string[]): Promise<Api.GetNetworkMetasResponse> {
        const response = await this.client.post<any, Api.GetNetworkMetasResponse>(`/machines/${this.machine_id}/networks/metas`, {
            instance_ids: instance_ids
        });
        return response;
    }
    async get_managed_config_revision(): Promise<string | null> {
        const response = await this.client.get<any, { config_revision?: string | null }>(
            `/machines/${this.machine_id}/managed-config-revision`,
        );
        const revision = response?.config_revision;
        return typeof revision === 'string' && revision.trim() ? revision : null;
    }
    async get_peer_conn_history(inst_id: string, hours: number): Promise<Api.PeerConnHistoryResponse | undefined> {
        const response = await this.client.get<any, Api.PeerConnHistoryResponse>(
            `/machines/${this.machine_id}/peer-history/${inst_id}`,
            { params: { hours } },
        );
        return response;
    }
    async get_logger_level(): Promise<string> {
        const response = await this.client.post<any, { level?: unknown }>(
            `/machines/${this.machine_id}/proxy-rpc`,
            {
                service_name: 'api.logger.LoggerRpcService',
                method_name: 'GetLoggerConfig',
                payload: {},
            },
        );
        return normalizeLoggerLevel(response.level);
    }
    async set_logger_level(level: string): Promise<void> {
        await this.client.post(
            `/machines/${this.machine_id}/proxy-rpc`,
            {
                service_name: 'api.logger.LoggerRpcService',
                method_name: 'SetLoggerConfig',
                payload: {
                    level: loggerLevelToRpc(level),
                },
            },
        );
    }
}

export default ApiClient;
