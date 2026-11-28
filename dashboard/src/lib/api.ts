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
  firewall?: { installed: boolean; pending: PendingInfo | null };
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
  /** New connections per second from one client, enforced by the firewall. */
  rate_limit?: number | null;
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
  challenge?: 'http-01' | 'tls-alpn-01' | 'dns-01' | null;
  renew_before?: string;
  dns?: AcmeDns | null;
}

export type DnsProvider = 'cloudflare' | 'rfc2136';
export type TsigAlgorithm = 'hmac-sha256' | 'hmac-sha384' | 'hmac-sha512';

/** Where dns-01 challenges set TXT records. Secrets stay in files. */
export interface AcmeDns {
  provider: DnsProvider;
  api_token_file?: string | null;
  api_url?: string | null;
  server?: string | null;
  zone?: string | null;
  key_name?: string | null;
  key_algorithm?: TsigAlgorithm;
  key_file?: string | null;
  propagation?: string;
}

export type RuleAction = 'allow' | 'deny';

export interface FirewallRule {
  name: string;
  action: RuleAction;
  /** Networks such as "203.0.113.0/24". Empty means any source. */
  sources?: string[];
  /** Ports (numbers) or ranges ("8000-8100"). Empty means every port. */
  ports?: (number | string)[];
  protocol?: Protocol;
  comment?: string;
}

export interface Blocklist {
  name: string;
  url: string;
  /** For example "12h". */
  refresh?: string;
  enabled?: boolean;
}

export interface AutoBan {
  enabled: boolean;
  threshold: number;
  window: string;
  ban_time: string;
}

export interface FirewallSettings {
  enabled: boolean;
  allow_lan: boolean;
  allow_ping: boolean;
  allow_docker_ports: boolean;
  rules?: FirewallRule[];
  allow?: string[];
  block?: string[];
  blocklists?: Blocklist[];
  countries?: string[];
  auto_ban: AutoBan;
}

export interface RouterSettings {
  upnp: boolean;
}

/** `'all'`, or the names of the services a peer may reach. */
export type Access = 'all' | string[];

export interface Peer {
  name: string;
  public_key: string;
  address: string;
  access?: Access;
}

export interface VpnSettings {
  enabled?: boolean;
  port?: number;
  subnet?: string;
  endpoint?: string;
  peers?: Peer[];
}

export interface DnsSettings {
  enabled?: boolean;
  zone?: string;
  lan?: boolean;
  upstream?: string[];
}

export interface VpnPeerInfo {
  name: string;
  address: string;
  public_key: string;
  endpoint: string | null;
  latest_handshake: number | null;
  online: boolean;
  rx_bytes: number;
  tx_bytes: number;
}

export interface VpnInfo {
  enabled: boolean;
  running: boolean;
  backend: 'kernel' | 'userspace' | null;
  interface: string;
  public_key: string | null;
  endpoint: string | null;
  address: string;
  subnet: string;
  port: number;
  problems: string[];
  peers: VpnPeerInfo[];
  dns: string[];
  names: { name: string; addresses: string[] }[];
}

export interface PeerConfigResponse {
  public_key: string;
  address: string;
  config: string;
  qr_svg: string;
}

export type RestartPolicy = 'no' | 'always' | 'unless-stopped' | 'on-failure';

export interface ManagedContainer {
  name: string;
  image: string;
  running?: boolean;
  command?: string[];
  env?: Record<string, string>;
  volumes?: string[];
  publish?: string[];
  devices?: string[];
  groups?: string[];
  restart?: RestartPolicy;
}

export interface Stack {
  name: string;
  compose: string;
  env?: Record<string, string>;
  running?: boolean;
}

export interface DockerContainerInfo {
  name: string;
  owner: { kind: 'container' } | { kind: 'stack'; stack: string; service: string };
  image: string;
  state: string;
  status: string;
  health: string | null;
  address: string | null;
  created: number | null;
}

export interface DockerInfo {
  enabled: boolean;
  available: boolean;
  error: string | null;
  subnet: string | null;
  containers: DockerContainerInfo[];
  stacks: { name: string; running: number; total: number; error: string | null }[];
  problems: string[];
}

export interface ContainerLogLine {
  time: string;
  stderr: boolean;
  text: string;
}

export interface ContainerStats {
  cpu_percent: number;
  memory_bytes: number;
  memory_limit: number;
  restarts: number;
  started_at: string | null;
}

export interface ImageUpdate {
  image: string;
  used_by: string[];
  update_available: boolean;
  error: string | null;
}

export interface DesiredState {
  containers?: ManagedContainer[];
  stacks?: Stack[];
  vpn?: VpnSettings;
  dns?: DnsSettings;
  interfaces?: Interface[];
  services?: Service[];
  routes?: Route[];
  acme?: AcmeSettings;
  limits?: Record<string, string | number>;
  firewall?: FirewallSettings;
  router?: RouterSettings;
  notifications?: NotificationSettings;
  ai?: AiSettings;
  updates?: UpdateSettings;
}

export type AutoInstall = 'off' | 'patch' | 'minor';

/** Updates of Hearthgate itself. Nothing installs by itself unless turned on. */
export interface UpdateSettings {
  check?: boolean;
  auto_install?: AutoInstall;
  /** Local time, HH:MM-HH:MM. */
  window?: string;
}

export type NoteKind = 'feature' | 'fix' | 'security' | 'breaking';
export interface ReleaseNote {
  kind: NoteKind;
  text: string;
}

export interface AvailableUpdate {
  version: string;
  published_at: string;
  bump: 'patch' | 'minor' | 'major';
  notes: ReleaseNote[];
  page: string;
  asset: string | null;
}

export type StepState = 'pending' | 'running' | 'done' | 'failed' | 'skipped';
export interface JobStep {
  title: string;
  detail: string;
  state: StepState;
}
export type JobOutcome = 'running' | 'installed' | 'rolled-back' | 'restored' | 'refused' | 'failed';
export interface UpdateJob {
  action: 'install' | 'rollback';
  from: string;
  to: string | null;
  steps: JobStep[];
  outcome: JobOutcome;
  detail: string | null;
  started_unix: number;
  finished_unix: number | null;
  log: string[];
}

export type InstallMethod = 'deb' | 'rpm' | 'tarball' | 'manual';
export interface UpdateInfo {
  current: string;
  method: InstallMethod;
  method_text: string;
  can_install: boolean;
  cannot_install: string | null;
  settings: Required<UpdateSettings>;
  checked_unix: number | null;
  error: string | null;
  available: AvailableUpdate | null;
  skipped: string[];
  scheduled: string | null;
  job: UpdateJob | null;
  rollback_to: string | null;
}

export type EventKind = 'certificates' | 'health' | 'security' | 'backups' | 'updates' | 'reviews';

export type AiProvider = 'anthropic' | 'openai' | 'gemini' | 'openai-compatible';

/** The optional AI assistant. Off unless turned on. */
export interface AiSettings {
  enabled?: boolean;
  provider?: AiProvider;
  model?: string | null;
  endpoint?: string | null;
  api_key_file?: string | null;
  /** Extra patterns (regular expressions) to hide before sending. */
  redact?: string[];
  weekly_review?: boolean;
  timeout?: string;
}

export interface AiProviderInfo {
  id: AiProvider;
  default_model: string | null;
  default_endpoint: string | null;
  needs_key: boolean;
}

export interface AiInfo {
  enabled: boolean;
  provider: AiProvider;
  model: string | null;
  endpoint: string | null;
  key_set: boolean;
  key_from_file: boolean;
  needs_key: boolean;
  providers: AiProviderInfo[];
  busy: boolean;
}

export type AiScope = { kind: 'all' } | { kind: 'firewall' } | { kind: 'service'; service: string };
export type AiSeverity = 'high' | 'medium' | 'low';

export interface PatchOp {
  op: 'add' | 'remove' | 'replace';
  path: string;
  value?: unknown;
}

export interface AiFinding {
  severity: AiSeverity;
  area: string;
  title: string;
  detail: string;
  fix?: { summary: string; changes: PatchOp[] };
}

export interface AiReview {
  id: string;
  created: string;
  created_unix: number;
  scope: AiScope;
  provider: AiProvider;
  model: string;
  scheduled: boolean;
  revision?: number | null;
  summary: string;
  findings: AiFinding[];
}

export interface AiPreview {
  url: string;
  body: unknown;
  redacted: number;
}

export interface Webhook {
  name: string;
  url: string;
  format?: 'json' | 'ntfy';
}

export interface EmailSettings {
  server: string;
  security?: 'starttls' | 'tls' | 'none';
  username?: string | null;
  password_file?: string | null;
  from: string;
  to: string[];
}

export interface NotificationSettings {
  /** Empty or missing: every kind. */
  events?: EventKind[];
  webhooks?: Webhook[];
  email?: EmailSettings | null;
}

export interface NotificationTestResult {
  target: string;
  ok: boolean;
  error?: string;
}

export interface PendingInfo {
  revision: number;
  seconds_left: number;
}

export interface FirewallInfo {
  installed: boolean;
  error: string | null;
  pending: PendingInfo | null;
  dropped: { total: number; blocklist: number; ban: number; policy: number; ratelimit: number };
  feed_entries: number;
  feeds: { name: string; entries: number; updated: number | null; error: string | null }[];
  bans: { address: string; seconds_left: number }[];
}

export interface RecentDrop {
  source: string;
  protocol: string;
  port: number;
  reason: 'blocklist' | 'ban' | 'policy' | 'rate-limit' | string;
  packets: number;
  last_seen_secs: number;
}

export interface RouterInfo {
  enabled: boolean;
  method: 'upnp' | 'nat-pmp' | null;
  router: string | null;
  local_ip: string | null;
  public_ip: string | null;
  double_nat: boolean;
  mappings: { port: number; protocol: string; active: boolean; error: string | null }[];
  error: string | null;
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
  ai: () => request<AiInfo>('GET', '/api/v1/ai'),
  setAiKey: (key: string) => request<void>('PUT', '/api/v1/ai/key', { key }),
  aiPreview: (scope: AiScope, settings?: AiSettings) =>
    request<AiPreview>('POST', '/api/v1/ai/preview', { scope, settings: settings ?? null }),
  aiReview: (scope: AiScope) => request<AiReview>('POST', '/api/v1/ai/review', { scope }),
  aiReviews: () => request<AiReview[]>('GET', '/api/v1/ai/reviews'),
  aiFixDraft: (review: string, finding: number) =>
    request<{ state: DesiredState; summary: string }>(
      'POST',
      `/api/v1/ai/reviews/${encodeURIComponent(review)}/findings/${finding}/draft`,
    ),
  updates: () => request<UpdateInfo>('GET', '/api/v1/updates'),
  checkUpdates: () => request<UpdateInfo>('POST', '/api/v1/updates/check'),
  installUpdate: (version: string, when: 'now' | 'window') =>
    request<UpdateInfo>('POST', '/api/v1/updates/install', { version, when }),
  unscheduleUpdate: () => request<UpdateInfo>('DELETE', '/api/v1/updates/scheduled'),
  rollbackUpdate: () => request<UpdateInfo>('POST', '/api/v1/updates/rollback'),
  skipUpdate: (version: string) => request<UpdateInfo>('POST', '/api/v1/updates/skip', { version }),
  unskipUpdate: (version: string) => request<UpdateInfo>('POST', '/api/v1/updates/unskip', { version }),
  updateHistory: () => request<UpdateJob[]>('GET', '/api/v1/updates/history'),
  testNotifications: (settings: NotificationSettings) =>
    request<NotificationTestResult[]>('POST', '/api/v1/notifications/test', settings),
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
  firewall: () => request<FirewallInfo>('GET', '/api/v1/firewall'),
  recentDrops: () => request<RecentDrop[]>('GET', '/api/v1/firewall/recent'),
  confirmFirewall: () => request<void>('POST', '/api/v1/firewall/confirm'),
  ban: (address: string, duration?: string) =>
    request<void>('POST', '/api/v1/firewall/bans', { address, duration: duration || null }),
  unban: (address: string) => request<void>('DELETE', `/api/v1/firewall/bans/${encodeURIComponent(address)}`),
  router: () => request<RouterInfo>('GET', '/api/v1/router'),
  vpn: () => request<VpnInfo>('GET', '/api/v1/vpn'),
  docker: () => request<DockerInfo>('GET', '/api/v1/docker'),
  restartContainer: (name: string) =>
    request<void>('POST', `/api/v1/docker/containers/${encodeURIComponent(name)}/restart`),
  updateContainer: (name: string) =>
    request<void>('POST', `/api/v1/docker/containers/${encodeURIComponent(name)}/update`),
  updateStack: (name: string) => request<void>('POST', `/api/v1/docker/stacks/${encodeURIComponent(name)}/update`),
  containerLogs: (name: string, tail = 200, search = '') =>
    request<ContainerLogLine[]>(
      'GET',
      `/api/v1/docker/containers/${encodeURIComponent(name)}/logs?tail=${tail}&search=${encodeURIComponent(search)}`,
    ),
  containerStats: (name: string) =>
    request<ContainerStats>('GET', `/api/v1/docker/containers/${encodeURIComponent(name)}/stats`),
  imageUpdates: () => request<ImageUpdate[]>('GET', '/api/v1/docker/updates'),
  peerConfig: (body: { name: string; access: Access; state: DesiredState }) =>
    request<PeerConfigResponse>('POST', '/api/v1/vpn/peer-config', body),
};
