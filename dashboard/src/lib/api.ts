// Typed client for Hearthgate's admin API (see /api/v1/openapi.json).
// Changes carry the session's CSRF token; a 401 sends the user back to
// the login screen.

export interface UserInfo {
  username: string;
  totp_enabled: boolean;
}

export interface SessionInfo {
  setup_required: boolean;
  user: UserInfo | null;
  csrf: string | null;
}

export interface ListenerStatus {
  socket: string;
  kind: string;
  open_connections: number;
}

export interface Rate {
  requests: number;
  failures: number;
}

export interface UpstreamStatus {
  service: string;
  port: number;
  endpoint: string;
  healthy: boolean;
  ejected: boolean;
  active: number;
  rate: Rate;
}

export interface HostInfo {
  hostname: string;
  os: string | null;
  uptime: number;
}

export interface StatusResponse {
  version: string;
  revision: number | null;
  latest_revision: number | null;
  listeners: ListenerStatus[];
  upstreams: UpstreamStatus[];
  problems: string[];
  host: HostInfo;
}

// Desired state, as in the OpenAPI document. Optional fields are filled
// with defaults by the server.

export type ListenerMode = 'https' | 'http' | 'tls-passthrough' | 'raw';
export type Protocol = 'tcp' | 'udp' | 'both';
export type Strategy = 'round-robin' | 'least-connections' | 'source-hash';
export type PortRef = number | string;

export interface Listener {
  port: number;
  mode: ListenerMode;
  protocol?: Protocol;
}

export interface Interface {
  name: string;
  domains?: string[];
  listeners: Listener[];
  bind?: string[];
  certificate?: string | null;
}

export interface HealthCheck {
  type: 'tcp' | 'http';
  path?: string;
  port?: PortRef | null;
  interval?: string;
  timeout?: string;
  healthy_threshold?: number;
  unhealthy_threshold?: number;
}

export interface Service {
  name: string;
  kind: 'external' | 'docker';
  host?: string | null;
  container?: string | null;
  network?: string | null;
  ports: Record<string, number>;
  health?: HealthCheck | null;
}

export interface Target {
  service: string;
  port: PortRef;
  weight?: number;
}

export interface Route {
  name: string;
  interface: string;
  listener: number;
  hosts?: string[];
  path_prefix?: string;
  strip_prefix?: boolean;
  strategy?: Strategy;
  targets: Target[];
}

export interface AcmeSettings {
  accept_terms?: boolean;
  email?: string | null;
  staging?: boolean;
  directory?: string | null;
  directory_root?: string | null;
  challenge?: 'http-01' | 'tls-alpn-01' | null;
  renew_before?: string;
}

export interface DesiredState {
  interfaces?: Interface[];
  services?: Service[];
  routes?: Route[];
  acme?: AcmeSettings;
  limits?: Record<string, string | number>;
}

export interface RevisionInfo {
  id: number;
  created_at: string;
  author: string;
  message: string;
  hash: string;
  rollback_of?: number | null;
}

export interface StateResponse {
  revision: RevisionInfo | null;
  state: DesiredState;
}

export interface ApplyResponse {
  changed: boolean;
  revision: RevisionInfo;
}

export interface PreviewResponse {
  valid: boolean;
  problems: string[];
  changed: boolean;
  diff: string;
}

export interface CertificateInfo {
  name: string;
  source: 'manual' | 'automatic';
  domains: string[];
  not_after: number;
  days_left: number;
  issuer: string;
}

export interface LogLine {
  id: number;
  time: number;
  level: 'error' | 'warn' | 'info' | 'debug' | 'trace';
  message: string;
}

export interface TrafficBucket {
  start: number;
  requests: number;
  failures: number;
}

export interface AccessSettings {
  mode: 'lan' | 'custom';
  port: number;
  listen: string[];
  allow: string[];
}

export interface AccessInfo {
  locked: boolean;
  settings: AccessSettings;
  effective: { listen: string[]; allow: string[] };
  detected: { listen: string[]; allow: string[] };
  https: boolean;
  fingerprint: string | null;
  client: string;
}

export interface ApiTokenInfo {
  id: number;
  name: string;
  created_at: number;
  last_used_at: number | null;
}

export interface CreatedToken extends ApiTokenInfo {
  token: string;
}

export interface TotpSecret {
  secret: string;
  uri: string;
}

export interface Problem {
  error: string;
  problems?: string[];
  totp_required?: boolean;
}

/** An error answer from the API, with its message ready to show. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly problem: Problem,
  ) {
    super(problem.error);
  }
}

let csrf: string | null = null;
let onUnauthorized: (() => void) | null = null;

export function setCsrf(token: string | null): void {
  csrf = token;
}

/** Called when the session has ended, so the app can show the login. */
export function setUnauthorizedHandler(handler: () => void): void {
  onUnauthorized = handler;
}

export async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { accept: 'application/json' };
  if (body !== undefined) headers['content-type'] = 'application/json';
  if (method !== 'GET' && csrf) headers['x-csrf-token'] = csrf;
  let response: Response;
  try {
    response = await fetch(path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      credentials: 'same-origin',
    });
  } catch {
    throw new ApiError(0, { error: "Can't reach Hearthgate. Check that the service is running." });
  }
  if (response.status === 204) return undefined as T;
  const text = await response.text();
  let data: unknown = null;
  try {
    data = text ? JSON.parse(text) : null;
  } catch {
    data = { error: text || response.statusText };
  }
  if (!response.ok) {
    const problem = (data as Problem) ?? { error: response.statusText };
    const isLogin = path === '/api/v1/auth/login' || path === '/api/v1/auth/setup';
    if (response.status === 401 && !isLogin) onUnauthorized?.();
    throw new ApiError(response.status, problem);
  }
  return data as T;
}

export const api = {
  session: () => request<SessionInfo>('GET', '/api/v1/auth/session'),
  setup: (setup_code: string, username: string, password: string) =>
    request<SessionInfo>('POST', '/api/v1/auth/setup', { setup_code, username, password }),
  login: (username: string, password: string, code?: string) =>
    request<SessionInfo>('POST', '/api/v1/auth/login', { username, password, code: code || null }),
  logout: () => request<void>('POST', '/api/v1/auth/logout'),
  status: () => request<StatusResponse>('GET', '/api/v1/status'),
  state: () => request<StateResponse>('GET', '/api/v1/state'),
  apply: (state: DesiredState, message: string) =>
    request<ApplyResponse>('PUT', '/api/v1/state', { state, message }),
  preview: (state: DesiredState) => request<PreviewResponse>('POST', '/api/v1/state/preview', state),
  revisions: (limit = 50) => request<RevisionInfo[]>('GET', `/api/v1/revisions?limit=${limit}`),
  revisionDiff: (id: number) =>
    request<{ previous: number | null; diff: string }>('GET', `/api/v1/revisions/${id}/diff`),
  rollback: (id: number) => request<ApplyResponse>('POST', `/api/v1/revisions/${id}/rollback`),
  certificates: () => request<CertificateInfo[]>('GET', '/api/v1/certificates'),
  logs: (limit = 200) => request<LogLine[]>('GET', `/api/v1/logs?limit=${limit}`),
  traffic: () => request<TrafficBucket[]>('GET', '/api/v1/traffic'),
  access: () => request<AccessInfo>('GET', '/api/v1/settings/admin-access'),
  setAccess: (settings: AccessSettings) =>
    request<AccessInfo>('PUT', '/api/v1/settings/admin-access', settings),
  changePassword: (current_password: string, new_password: string) =>
    request<void>('POST', '/api/v1/auth/password', { current_password, new_password }),
  totpBegin: () => request<TotpSecret>('POST', '/api/v1/auth/totp/begin'),
  totpEnable: (secret: string, code: string) =>
    request<void>('POST', '/api/v1/auth/totp/enable', { secret, code }),
  totpDisable: (password: string) => request<void>('POST', '/api/v1/auth/totp/disable', { password }),
  tokens: () => request<ApiTokenInfo[]>('GET', '/api/v1/tokens'),
  createToken: (name: string) => request<CreatedToken>('POST', '/api/v1/tokens', { name }),
  deleteToken: (id: number) => request<void>('DELETE', `/api/v1/tokens/${id}`),
};
