import type {
	AddMemberRequest,
	AdminUserDetail,
	AdminUserList,
	Allocation,
	AllocationList,
	AllOperationsResponse,
	AuditLogPage,
	AuditLogQuery,
	Backup,
	BackupListResponse,
	BackupSchedule,
	BulkDeleteBackupsRequest,
	BulkDeleteBackupsResponse,
	ChangePasswordRequest,
	ContentDependentsResponse,
	ContentIdsRequest,
	ContentInstallRequest,
	ContentInstallResponse,
	ContentListResponse,
	ContentMutationResponse,
	ContentUpdateRequest,
	ContentUpdateResponse,
	ContentUploadResponse,
	CrashAnalysisRequest,
	CrashAnalysisResponse,
	CreateAllocationRequest,
	CreateBackupRequest,
	CreateItemRequest,
	CreateItemResponse,
	CreateServerRequest,
	CreateServerResponse,
	CreateUserRequest,
	DeleteItemQuery,
	DeleteUserServers,
	ExtractDryRunResponse,
	ExtractRequest,
	FilePath,
	FilesMetaResponse,
	GameVersionChangeRequest,
	GameVersionList,
	GameVersionPreviewResponse,
	HostCapacity,
	InstallAccepted,
	InstallRequest,
	InvitationList,
	JavaRuntimeList,
	JavaRuntimeOverview,
	ListDirectoryQuery,
	ListDirectoryResponse,
	LoaderBuildList,
	LoaderId,
	LoaderList,
	LogFileContentResponse,
	LogFileListResponse,
	LoginRequest,
	Me,
	ModpackContentsResponse,
	ModpackInstallRequest,
	ModpackUnlinkResponse,
	ModpackUpdateRequest,
	MoveItemRequest,
	MoveItemResponse,
	Operation,
	OperationAccepted,
	OperationListResponse,
	PanelSettings,
	PanelUser,
	PowerRequest,
	PowerResponse,
	ReadContentQuery,
	ReinviteResponse,
	RenameAllocationRequest,
	RenameBackupRequest,
	ResetRequest,
	ResetToSetupResponse,
	RestoreBackupRequest,
	RestoreBackupResponse,
	RetryBackupResponse,
	SendCommandRequest,
	Server,
	ServerListResponse,
	ServerMember,
	ServerMemberList,
	ServerProperties,
	ServerPropertiesPatch,
	SetPrimaryResponse,
	StartupOptions,
	StartupOptionsPatch,
	Ulid,
	UpdateBackupScheduleRequest,
	UpdateMemberRequest,
	UpdateServerRequest,
	UpdateUserRequest,
	UserLimits,
	UserLimitsResponse,
	UserSearchResponse,
	WriteContentQuery,
} from './types'

export const API_BASE = '/api/v1'
export const MODRINTH_PROXY_BASE = `${API_BASE}/modrinth`
export const BACKUP_ALIAS_BASE = '/modrinth/v0/backups'

export const FILE_LIST_MAX_ITEMS = 20_000

export class ApiRequestError extends Error {
	readonly status: number
	readonly code: string
	readonly retryAfterSeconds: number | null

	constructor(
		status: number,
		code: string,
		message: string,
		retryAfterSeconds: number | null = null,
	) {
		super(message)
		this.name = 'ApiRequestError'
		this.status = status
		this.code = code
		this.retryAfterSeconds = retryAfterSeconds
	}
}

export function isApiRequestError(value: unknown): value is ApiRequestError {
	return value instanceof ApiRequestError
}

export function hasErrorCode(value: unknown, ...codes: string[]): boolean {
	return isApiRequestError(value) && codes.includes(value.code)
}

let unauthenticatedHandler: (() => void) | null = null
let unauthenticatedReported = false

export function setUnauthenticatedHandler(handler: (() => void) | null): void {
	unauthenticatedHandler = handler
	unauthenticatedReported = false
}

function reportUnauthenticated(): void {
	if (unauthenticatedReported) return
	unauthenticatedReported = true
	unauthenticatedHandler?.()
}

type QueryValue = string | number | boolean | string[] | number[] | null | undefined
export type QueryParams = Record<string, QueryValue>

export function buildUrl(path: string, query?: QueryParams, base: string = API_BASE): string {
	const search = new URLSearchParams()
	for (const [key, value] of Object.entries(query ?? {})) {
		if (value === undefined || value === null) continue
		if (Array.isArray(value)) {
			for (const entry of value) search.append(key, String(entry))
		} else {
			search.append(key, String(value))
		}
	}
	const suffix = search.toString()
	return suffix ? `${base}${path}?${suffix}` : `${base}${path}`
}

function segment(value: string | number): string {
	return encodeURIComponent(String(value))
}

function parseRetryAfter(header: string | null): number | null {
	if (!header) return null
	const seconds = Number(header)
	return Number.isFinite(seconds) ? seconds : null
}

function envelopeFrom(raw: string): { error: string; message: string } | null {
	let parsed: unknown
	try {
		parsed = JSON.parse(raw)
	} catch {
		return null
	}
	if (typeof parsed !== 'object' || parsed === null) return null
	if (!('error' in parsed) || !('message' in parsed)) return null
	const { error, message } = parsed
	if (typeof error !== 'string' || typeof message !== 'string') return null
	return { error, message }
}

function errorFrom(status: number, raw: string, retryAfterSeconds: number | null): ApiRequestError {
	const envelope = envelopeFrom(raw)
	const code = envelope?.error ?? (status === 401 ? 'unauthenticated' : `http_${status}`)
	const message = envelope?.message ?? `Request failed with status ${status}`
	if (status === 401 && code === 'unauthenticated') reportUnauthenticated()
	return new ApiRequestError(status, code, message, retryAfterSeconds)
}

interface SendOptions {
	method: string
	query?: QueryParams
	json?: unknown
	body?: BodyInit
	headers?: Record<string, string>
	signal?: AbortSignal
}

async function send(
	path: string,
	options: SendOptions,
	base: string = API_BASE,
): Promise<Response> {
	const headers: Record<string, string> = { ...options.headers }
	let body = options.body
	if (options.json !== undefined) {
		headers['content-type'] = 'application/json'
		body = JSON.stringify(options.json)
	}

	let response: Response
	try {
		response = await fetch(buildUrl(path, options.query, base), {
			method: options.method,
			credentials: 'same-origin',
			headers,
			body,
			signal: options.signal,
		})
	} catch (cause) {
		if (options.signal?.aborted) throw cause
		throw new ApiRequestError(0, 'network_unreachable', 'The panel could not be reached.')
	}

	if (!response.ok) {
		throw errorFrom(
			response.status,
			await response.text().catch(() => ''),
			parseRetryAfter(response.headers.get('retry-after')),
		)
	}
	unauthenticatedReported = false
	return response
}

async function requestJson<T>(path: string, options: SendOptions): Promise<T> {
	const response = await send(path, options)
	return (await response.json()) as T
}

async function requestVoid(path: string, options: SendOptions): Promise<void> {
	await send(path, options)
}

async function requestBlob(path: string, options: SendOptions): Promise<Blob> {
	const response = await send(path, options)
	return await response.blob()
}

async function requestText(path: string, options: SendOptions): Promise<string> {
	const response = await send(path, options)
	return await response.text()
}

export interface UploadProgress {
	loaded: number
	total: number
	progress: number
}

export interface UploadHandle<T> {
	promise: Promise<T>
	onProgress: (callback: (progress: UploadProgress) => void) => UploadHandle<T>
	cancel: () => void
}

interface UploadOptions {
	method: string
	query?: QueryParams
	body: XMLHttpRequestBodyInit
	contentType?: string
	signal?: AbortSignal
}

function upload<T>(
	path: string,
	options: UploadOptions,
	decode: (raw: string) => T,
): UploadHandle<T> {
	const listeners: Array<(progress: UploadProgress) => void> = []
	const request = new XMLHttpRequest()

	const promise = new Promise<T>((resolve, reject) => {
		request.open(options.method, buildUrl(path, options.query))
		request.withCredentials = true
		if (options.contentType) request.setRequestHeader('content-type', options.contentType)

		request.upload.addEventListener('progress', (event) => {
			const total = event.lengthComputable ? event.total : 0
			const progress = total > 0 ? event.loaded / total : 0
			for (const listener of listeners) listener({ loaded: event.loaded, total, progress })
		})
		request.addEventListener('load', () => {
			if (request.status >= 200 && request.status < 300) {
				unauthenticatedReported = false
				resolve(decode(request.responseText))
				return
			}
			reject(
				errorFrom(
					request.status,
					request.responseText,
					parseRetryAfter(request.getResponseHeader('retry-after')),
				),
			)
		})
		request.addEventListener('error', () => {
			reject(new ApiRequestError(0, 'network_unreachable', 'The panel could not be reached.'))
		})
		request.addEventListener('abort', () => {
			reject(new DOMException('Upload cancelled', 'AbortError'))
		})

		if (options.signal?.aborted) {
			reject(new DOMException('Upload cancelled', 'AbortError'))
			return
		}
		options.signal?.addEventListener('abort', () => request.abort(), { once: true })
		request.send(options.body)
	})

	const handle: UploadHandle<T> = {
		promise,
		onProgress(callback) {
			listeners.push(callback)
			return handle
		},
		cancel() {
			request.abort()
		},
	}
	return handle
}

function uploadJson<T>(path: string, options: UploadOptions): UploadHandle<T> {
	return upload<T>(path, options, (raw) => JSON.parse(raw) as T)
}

function uploadNothing(path: string, options: UploadOptions): UploadHandle<void> {
	return upload<void>(path, options, () => undefined)
}

export type OffsetPageQuery = { limit?: number; offset?: number }
export type BeforePageQuery = { limit?: number; before?: Ulid }
export type AfterPageQuery = { page_size?: number; after?: string }

export type ServerListQuery = { scope?: 'visible' | 'all' }
export type DeleteServerQuery = { keep_backups?: boolean }
export type RetryBackupQuery = { acknowledge_abuse?: boolean }
export type UserSearchQuery = { query: string; limit?: number }
export type AllOperationsQuery = BeforePageQuery & { state?: 'active' | 'all'; server_id?: Ulid[] }
export type ServerOperationsQuery = BeforePageQuery & {
	state?: 'active' | 'all'
	include_dismissed?: boolean
}
export type LogFileListQuery = OffsetPageQuery
export type ContentListQuery = { refresh_updates?: boolean }
export type GameVersionPreviewQuery = {
	game_version: string
	loader?: LoaderId
	loader_version?: string
}
export type JavaRuntimeQuery = { server_id?: Ulid }
export type AdminUserListQuery = OffsetPageQuery & { query?: string }
export type DeleteUserQuery = { servers?: DeleteUserServers; transfer_to?: Ulid }

interface Cancellable {
	signal?: AbortSignal
}

const serverPath = (serverId: Ulid, suffix: string) => `/servers/${segment(serverId)}${suffix}`

const auth = {
	login: (body: LoginRequest, options: Cancellable = {}) =>
		requestJson<Me>('/auth/login', { method: 'POST', json: body, signal: options.signal }),

	logout: (options: Cancellable = {}) =>
		requestVoid('/auth/logout', { method: 'POST', signal: options.signal }),

	me: (options: Cancellable = {}) =>
		requestJson<Me>('/me', { method: 'GET', signal: options.signal }),

	changePassword: (body: ChangePasswordRequest, options: Cancellable = {}) =>
		requestVoid('/me/password', { method: 'POST', json: body, signal: options.signal }),

	searchUsers: (query: UserSearchQuery, options: Cancellable = {}) =>
		requestJson<UserSearchResponse>('/users/search', {
			method: 'GET',
			query,
			signal: options.signal,
		}),
}

const servers = {
	list: (query: ServerListQuery = {}, options: Cancellable = {}) =>
		requestJson<ServerListResponse>('/servers', { method: 'GET', query, signal: options.signal }),

	create: (body: CreateServerRequest, options: Cancellable = {}) =>
		requestJson<CreateServerResponse>('/servers', {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	get: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<Server>(serverPath(serverId, ''), { method: 'GET', signal: options.signal }),

	update: (serverId: Ulid, body: UpdateServerRequest, options: Cancellable = {}) =>
		requestJson<Server>(serverPath(serverId, ''), {
			method: 'PATCH',
			json: body,
			signal: options.signal,
		}),

	remove: (serverId: Ulid, query: DeleteServerQuery = {}, options: Cancellable = {}) =>
		requestJson<OperationAccepted>(serverPath(serverId, ''), {
			method: 'DELETE',
			query,
			signal: options.signal,
		}),

	power: (serverId: Ulid, body: PowerRequest, options: Cancellable = {}) =>
		requestJson<PowerResponse>(serverPath(serverId, '/power'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),
}

const operations = {
	listAll: (query: AllOperationsQuery = {}, options: Cancellable = {}) =>
		requestJson<AllOperationsResponse>('/operations', {
			method: 'GET',
			query,
			signal: options.signal,
		}),

	list: (serverId: Ulid, query: ServerOperationsQuery = {}, options: Cancellable = {}) =>
		requestJson<OperationListResponse>(serverPath(serverId, '/operations'), {
			method: 'GET',
			query,
			signal: options.signal,
		}),

	get: (serverId: Ulid, operationId: Ulid, options: Cancellable = {}) =>
		requestJson<Operation>(serverPath(serverId, `/operations/${segment(operationId)}`), {
			method: 'GET',
			signal: options.signal,
		}),

	cancel: (serverId: Ulid, operationId: Ulid, options: Cancellable = {}) =>
		requestJson<Operation>(serverPath(serverId, `/operations/${segment(operationId)}/cancel`), {
			method: 'POST',
			signal: options.signal,
		}),

	dismiss: (serverId: Ulid, operationId: Ulid, options: Cancellable = {}) =>
		requestVoid(serverPath(serverId, `/operations/${segment(operationId)}/dismiss`), {
			method: 'POST',
			signal: options.signal,
		}),

	retry: (serverId: Ulid, operationId: Ulid, options: Cancellable = {}) =>
		requestJson<OperationAccepted>(
			serverPath(serverId, `/operations/${segment(operationId)}/retry`),
			{
				method: 'POST',
				signal: options.signal,
			},
		),

	putPayload: (serverId: Ulid, operationId: Ulid, file: Blob, options: Cancellable = {}) =>
		uploadJson<OperationAccepted>(
			serverPath(serverId, `/operations/${segment(operationId)}/payload`),
			{
				method: 'PUT',
				body: file,
				contentType: 'application/octet-stream',
				signal: options.signal,
			},
		),
}

const consoleApi = {
	sendCommand: (serverId: Ulid, body: SendCommandRequest, options: Cancellable = {}) =>
		requestVoid(serverPath(serverId, '/console/command'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	clear: (serverId: Ulid, options: Cancellable = {}) =>
		requestVoid(serverPath(serverId, '/console/clear'), { method: 'POST', signal: options.signal }),

	crashAnalysis: (serverId: Ulid, body: CrashAnalysisRequest = {}, options: Cancellable = {}) =>
		requestJson<CrashAnalysisResponse>(serverPath(serverId, '/console/crash-analysis'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	listLogs: (serverId: Ulid, query: LogFileListQuery = {}, options: Cancellable = {}) =>
		requestJson<LogFileListResponse>(serverPath(serverId, '/console/logs'), {
			method: 'GET',
			query,
			signal: options.signal,
		}),

	readLog: (serverId: Ulid, file: FilePath, options: Cancellable = {}) =>
		requestJson<LogFileContentResponse>(serverPath(serverId, '/console/logs/content'), {
			method: 'GET',
			query: { file },
			signal: options.signal,
		}),

	deleteLog: (serverId: Ulid, file: FilePath, options: Cancellable = {}) =>
		requestVoid(serverPath(serverId, '/console/logs'), {
			method: 'DELETE',
			query: { file },
			signal: options.signal,
		}),
}

const files = {
	meta: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<FilesMetaResponse>(serverPath(serverId, '/files/meta'), {
			method: 'GET',
			signal: options.signal,
		}),

	list: (serverId: Ulid, query: ListDirectoryQuery = {}, options: Cancellable = {}) =>
		requestJson<ListDirectoryResponse>(serverPath(serverId, '/files/list'), {
			method: 'GET',
			query: { ...query },
			signal: options.signal,
		}),

	listAll: async (
		serverId: Ulid,
		path: FilePath = '/',
		options: Cancellable & { pageSize?: number } = {},
	) => {
		let page = await files.list(serverId, { path, page_size: options.pageSize }, options)
		const items = [...page.items]
		while (page.has_more && page.next_after !== null && items.length < FILE_LIST_MAX_ITEMS) {
			page = await files.list(
				serverId,
				{ path, after: page.next_after, page_size: options.pageSize },
				options,
			)
			items.push(...page.items)
		}
		return { path: page.path, items, truncated: page.has_more }
	},

	create: (serverId: Ulid, body: CreateItemRequest, options: Cancellable = {}) =>
		requestJson<CreateItemResponse>(serverPath(serverId, '/files/create'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	move: (serverId: Ulid, body: MoveItemRequest, options: Cancellable = {}) =>
		requestJson<MoveItemResponse>(serverPath(serverId, '/files/move'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	remove: (serverId: Ulid, query: DeleteItemQuery, options: Cancellable = {}) =>
		requestVoid(serverPath(serverId, '/files'), {
			method: 'DELETE',
			query: { ...query },
			signal: options.signal,
		}),

	readText: (serverId: Ulid, query: ReadContentQuery, options: Cancellable = {}) =>
		requestText(serverPath(serverId, '/files/content'), {
			method: 'GET',
			query: { ...query },
			signal: options.signal,
		}),

	readBlob: (serverId: Ulid, query: ReadContentQuery, options: Cancellable = {}) =>
		requestBlob(serverPath(serverId, '/files/content'), {
			method: 'GET',
			query: { ...query },
			signal: options.signal,
		}),

	contentUrl: (serverId: Ulid, query: ReadContentQuery) =>
		buildUrl(serverPath(serverId, '/files/content'), { ...query }),

	write: (
		serverId: Ulid,
		query: WriteContentQuery,
		body: Blob | ArrayBuffer | string,
		options: Cancellable = {},
	) =>
		requestVoid(serverPath(serverId, '/files/content'), {
			method: 'PUT',
			query: { ...query },
			body,
			headers: { 'content-type': 'application/octet-stream' },
			signal: options.signal,
		}),

	upload: (serverId: Ulid, query: WriteContentQuery, file: Blob, options: Cancellable = {}) =>
		uploadNothing(serverPath(serverId, '/files/content'), {
			method: 'PUT',
			query: { ...query },
			body: file,
			contentType: 'application/octet-stream',
			signal: options.signal,
		}),

	extractDryRun: (serverId: Ulid, body: Omit<ExtractRequest, 'dry'>, options: Cancellable = {}) =>
		requestJson<ExtractDryRunResponse>(serverPath(serverId, '/files/extract'), {
			method: 'POST',
			json: { ...body, dry: true },
			signal: options.signal,
		}),

	extract: (serverId: Ulid, body: Omit<ExtractRequest, 'dry'>, options: Cancellable = {}) =>
		requestJson<OperationAccepted>(serverPath(serverId, '/files/extract'), {
			method: 'POST',
			json: { ...body, dry: false },
			signal: options.signal,
		}),
}

const content = {
	list: (serverId: Ulid, query: ContentListQuery = {}, options: Cancellable = {}) =>
		requestJson<ContentListResponse>(serverPath(serverId, '/content'), {
			method: 'GET',
			query,
			signal: options.signal,
		}),

	modpackContents: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<ModpackContentsResponse>(serverPath(serverId, '/content/modpack/contents'), {
			method: 'GET',
			signal: options.signal,
		}),

	enable: (serverId: Ulid, body: ContentIdsRequest, options: Cancellable = {}) =>
		requestJson<ContentMutationResponse>(serverPath(serverId, '/content/enable'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	disable: (serverId: Ulid, body: ContentIdsRequest, options: Cancellable = {}) =>
		requestJson<ContentMutationResponse>(serverPath(serverId, '/content/disable'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	remove: (serverId: Ulid, body: ContentIdsRequest, options: Cancellable = {}) =>
		requestJson<ContentMutationResponse>(serverPath(serverId, '/content/delete'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	update: (serverId: Ulid, body: ContentUpdateRequest, options: Cancellable = {}) =>
		requestJson<ContentUpdateResponse>(serverPath(serverId, '/content/update'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	install: (serverId: Ulid, body: ContentInstallRequest, options: Cancellable = {}) =>
		requestJson<ContentInstallResponse>(serverPath(serverId, '/content/install'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	upload: (serverId: Ulid, selection: File[], options: Cancellable = {}) => {
		const form = new FormData()
		for (const file of selection) form.append('file', file, file.name)
		return uploadJson<ContentUploadResponse>(serverPath(serverId, '/content/upload'), {
			method: 'POST',
			body: form,
			signal: options.signal,
		})
	},

	dependents: (serverId: Ulid, body: ContentIdsRequest, options: Cancellable = {}) =>
		requestJson<ContentDependentsResponse>(serverPath(serverId, '/content/dependents'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	installModpack: (serverId: Ulid, body: ModpackInstallRequest, options: Cancellable = {}) =>
		requestJson<OperationAccepted>(serverPath(serverId, '/content/modpack/install'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	uploadModpack: (
		serverId: Ulid,
		file: File,
		keepExtraContent: boolean,
		options: Cancellable = {},
	) => {
		const form = new FormData()
		form.append('file', file, file.name)
		form.append(
			'meta',
			JSON.stringify({ source: { kind: 'upload' }, keep_extra_content: keepExtraContent }),
		)
		return uploadJson<OperationAccepted>(serverPath(serverId, '/content/modpack/install'), {
			method: 'POST',
			body: form,
			signal: options.signal,
		})
	},

	updateModpack: (serverId: Ulid, body: ModpackUpdateRequest, options: Cancellable = {}) =>
		requestJson<OperationAccepted>(serverPath(serverId, '/content/modpack/update'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	unlinkModpack: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<ModpackUnlinkResponse>(serverPath(serverId, '/content/modpack/unlink'), {
			method: 'POST',
			signal: options.signal,
		}),

	previewGameVersion: (serverId: Ulid, query: GameVersionPreviewQuery, options: Cancellable = {}) =>
		requestJson<GameVersionPreviewResponse>(serverPath(serverId, '/content/game-version/preview'), {
			method: 'GET',
			query,
			signal: options.signal,
		}),

	changeGameVersion: (serverId: Ulid, body: GameVersionChangeRequest, options: Cancellable = {}) =>
		requestJson<OperationAccepted>(serverPath(serverId, '/content/game-version'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),
}

const settings = {
	properties: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<ServerProperties>(serverPath(serverId, '/properties'), {
			method: 'GET',
			signal: options.signal,
		}),

	patchProperties: (serverId: Ulid, body: ServerPropertiesPatch, options: Cancellable = {}) =>
		requestJson<ServerProperties>(serverPath(serverId, '/properties'), {
			method: 'PATCH',
			json: body,
			signal: options.signal,
		}),

	startup: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<StartupOptions>(serverPath(serverId, '/startup'), {
			method: 'GET',
			signal: options.signal,
		}),

	patchStartup: (serverId: Ulid, body: StartupOptionsPatch, options: Cancellable = {}) =>
		requestJson<StartupOptions>(serverPath(serverId, '/startup'), {
			method: 'PATCH',
			json: body,
			signal: options.signal,
		}),

	javaRuntimes: (query: JavaRuntimeQuery = {}, options: Cancellable = {}) =>
		requestJson<JavaRuntimeList>('/java-runtimes', {
			method: 'GET',
			query,
			signal: options.signal,
		}),

	allocations: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<AllocationList>(serverPath(serverId, '/allocations'), {
			method: 'GET',
			signal: options.signal,
		}),

	createAllocation: (serverId: Ulid, body: CreateAllocationRequest, options: Cancellable = {}) =>
		requestJson<Allocation>(serverPath(serverId, '/allocations'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	renameAllocation: (
		serverId: Ulid,
		port: number,
		body: RenameAllocationRequest,
		options: Cancellable = {},
	) =>
		requestJson<Allocation>(serverPath(serverId, `/allocations/${segment(port)}`), {
			method: 'PATCH',
			json: body,
			signal: options.signal,
		}),

	deleteAllocation: (serverId: Ulid, port: number, options: Cancellable = {}) =>
		requestVoid(serverPath(serverId, `/allocations/${segment(port)}`), {
			method: 'DELETE',
			signal: options.signal,
		}),

	setPrimaryAllocation: (serverId: Ulid, port: number, options: Cancellable = {}) =>
		requestJson<SetPrimaryResponse>(serverPath(serverId, `/allocations/${segment(port)}/primary`), {
			method: 'PUT',
			signal: options.signal,
		}),

	loaders: (options: Cancellable = {}) =>
		requestJson<LoaderList>('/loaders', { method: 'GET', signal: options.signal }),

	gameVersions: (loader: LoaderId, options: Cancellable = {}) =>
		requestJson<GameVersionList>(`/loaders/${segment(loader)}/game-versions`, {
			method: 'GET',
			signal: options.signal,
		}),

	builds: (loader: LoaderId, gameVersion: string, options: Cancellable = {}) =>
		requestJson<LoaderBuildList>(
			`/loaders/${segment(loader)}/game-versions/${segment(gameVersion)}/builds`,
			{
				method: 'GET',
				signal: options.signal,
			},
		),

	install: (serverId: Ulid, body: InstallRequest, options: Cancellable = {}) =>
		requestJson<InstallAccepted>(serverPath(serverId, '/install'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	repair: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<OperationAccepted>(serverPath(serverId, '/repair'), {
			method: 'POST',
			signal: options.signal,
		}),

	reset: (serverId: Ulid, body: ResetRequest, options: Cancellable = {}) =>
		requestJson<OperationAccepted>(serverPath(serverId, '/reset'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	resetToSetup: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<ResetToSetupResponse>(serverPath(serverId, '/reset-to-setup'), {
			method: 'POST',
			signal: options.signal,
		}),
}

const backups = {
	list: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<BackupListResponse>(serverPath(serverId, '/backups'), {
			method: 'GET',
			signal: options.signal,
		}),

	create: (serverId: Ulid, body: CreateBackupRequest, options: Cancellable = {}) =>
		requestJson<Backup>(serverPath(serverId, '/backups'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	rename: (serverId: Ulid, backupId: Ulid, body: RenameBackupRequest, options: Cancellable = {}) =>
		requestJson<Backup>(serverPath(serverId, `/backups/${segment(backupId)}`), {
			method: 'PATCH',
			json: body,
			signal: options.signal,
		}),

	remove: (serverId: Ulid, backupId: Ulid, options: Cancellable = {}) =>
		requestVoid(serverPath(serverId, `/backups/${segment(backupId)}`), {
			method: 'DELETE',
			signal: options.signal,
		}),

	bulkDelete: (serverId: Ulid, body: BulkDeleteBackupsRequest, options: Cancellable = {}) =>
		requestJson<BulkDeleteBackupsResponse>(serverPath(serverId, '/backups/bulk-delete'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	restore: (
		serverId: Ulid,
		backupId: Ulid,
		body: RestoreBackupRequest,
		options: Cancellable = {},
	) =>
		requestJson<RestoreBackupResponse>(
			serverPath(serverId, `/backups/${segment(backupId)}/restore`),
			{
				method: 'POST',
				json: body,
				signal: options.signal,
			},
		),

	retry: (
		serverId: Ulid,
		backupId: Ulid,
		query: RetryBackupQuery = {},
		options: Cancellable = {},
	) =>
		requestJson<RetryBackupResponse>(serverPath(serverId, `/backups/${segment(backupId)}/retry`), {
			method: 'POST',
			query,
			signal: options.signal,
		}),

	downloadUrl: (serverId: Ulid, backupId: Ulid) =>
		buildUrl(serverPath(serverId, `/backups/${segment(backupId)}/download`)),

	aliasDownloadUrl: (backupId: Ulid) =>
		buildUrl(`/${segment(backupId)}/download`, undefined, BACKUP_ALIAS_BASE),

	schedule: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<BackupSchedule>(serverPath(serverId, '/backups/schedule'), {
			method: 'GET',
			signal: options.signal,
		}),

	setSchedule: (serverId: Ulid, body: UpdateBackupScheduleRequest, options: Cancellable = {}) =>
		requestJson<BackupSchedule>(serverPath(serverId, '/backups/schedule'), {
			method: 'PUT',
			json: body,
			signal: options.signal,
		}),
}

const access = {
	members: (serverId: Ulid, options: Cancellable = {}) =>
		requestJson<ServerMemberList>(serverPath(serverId, '/members'), {
			method: 'GET',
			signal: options.signal,
		}),

	addMember: (serverId: Ulid, body: AddMemberRequest, options: Cancellable = {}) =>
		requestJson<ServerMember>(serverPath(serverId, '/members'), {
			method: 'POST',
			json: body,
			signal: options.signal,
		}),

	updateMember: (
		serverId: Ulid,
		userId: Ulid,
		body: UpdateMemberRequest,
		options: Cancellable = {},
	) =>
		requestJson<ServerMember>(serverPath(serverId, `/members/${segment(userId)}`), {
			method: 'PATCH',
			json: body,
			signal: options.signal,
		}),

	removeMember: (serverId: Ulid, userId: Ulid, options: Cancellable = {}) =>
		requestVoid(serverPath(serverId, `/members/${segment(userId)}`), {
			method: 'DELETE',
			signal: options.signal,
		}),

	reinvite: (serverId: Ulid, userId: Ulid, options: Cancellable = {}) =>
		requestJson<ReinviteResponse>(serverPath(serverId, `/members/${segment(userId)}/reinvite`), {
			method: 'POST',
			signal: options.signal,
		}),

	invitations: (options: Cancellable = {}) =>
		requestJson<InvitationList>('/invitations', { method: 'GET', signal: options.signal }),

	acceptInvitation: (invitationId: Ulid, options: Cancellable = {}) =>
		requestJson<ServerMember>(`/invitations/${segment(invitationId)}/accept`, {
			method: 'POST',
			signal: options.signal,
		}),

	declineInvitation: (invitationId: Ulid, options: Cancellable = {}) =>
		requestVoid(`/invitations/${segment(invitationId)}/decline`, {
			method: 'POST',
			signal: options.signal,
		}),

	auditLog: (serverId: Ulid, query: AuditLogQuery = {}, options: Cancellable = {}) =>
		requestJson<AuditLogPage>(serverPath(serverId, '/audit-log'), {
			method: 'GET',
			query: { ...query },
			signal: options.signal,
		}),
}

const admin = {
	host: (options: Cancellable = {}) =>
		requestJson<HostCapacity>('/admin/host', { method: 'GET', signal: options.signal }),

	users: (query: AdminUserListQuery = {}, options: Cancellable = {}) =>
		requestJson<AdminUserList>('/admin/users', { method: 'GET', query, signal: options.signal }),

	createUser: (body: CreateUserRequest, options: Cancellable = {}) =>
		requestJson<PanelUser>('/admin/users', { method: 'POST', json: body, signal: options.signal }),

	user: (userId: Ulid, options: Cancellable = {}) =>
		requestJson<AdminUserDetail>(`/admin/users/${segment(userId)}`, {
			method: 'GET',
			signal: options.signal,
		}),

	updateUser: (userId: Ulid, body: UpdateUserRequest, options: Cancellable = {}) =>
		requestJson<AdminUserDetail>(`/admin/users/${segment(userId)}`, {
			method: 'PATCH',
			json: body,
			signal: options.signal,
		}),

	deleteUser: (userId: Ulid, query: DeleteUserQuery = {}, options: Cancellable = {}) =>
		requestVoid(`/admin/users/${segment(userId)}`, {
			method: 'DELETE',
			query,
			signal: options.signal,
		}),

	limits: (userId: Ulid, options: Cancellable = {}) =>
		requestJson<UserLimitsResponse>(`/admin/users/${segment(userId)}/limits`, {
			method: 'GET',
			signal: options.signal,
		}),

	setLimits: (userId: Ulid, body: UserLimits, options: Cancellable = {}) =>
		requestJson<UserLimitsResponse>(`/admin/users/${segment(userId)}/limits`, {
			method: 'PUT',
			json: body,
			signal: options.signal,
		}),

	retrySystemUser: (userId: Ulid, options: Cancellable = {}) =>
		requestJson<AdminUserDetail>(`/admin/users/${segment(userId)}/system-user/retry`, {
			method: 'POST',
			signal: options.signal,
		}),

	settings: (options: Cancellable = {}) =>
		requestJson<PanelSettings>('/admin/settings', { method: 'GET', signal: options.signal }),

	setSettings: (body: PanelSettings, options: Cancellable = {}) =>
		requestJson<PanelSettings>('/admin/settings', {
			method: 'PUT',
			json: body,
			signal: options.signal,
		}),

	javaRuntimes: (options: Cancellable = {}) =>
		requestJson<JavaRuntimeOverview>('/admin/java-runtimes', {
			method: 'GET',
			signal: options.signal,
		}),

	fetchJavaRuntime: (major: number, options: Cancellable = {}) =>
		requestJson<JavaRuntimeOverview>(`/admin/java-runtimes/${segment(String(major))}`, {
			method: 'POST',
			signal: options.signal,
		}),

	removeJavaRuntime: (major: number, options: Cancellable = {}) =>
		requestJson<JavaRuntimeOverview>(`/admin/java-runtimes/${segment(String(major))}`, {
			method: 'DELETE',
			signal: options.signal,
		}),
}

export const api = {
	auth,
	servers,
	operations,
	console: consoleApi,
	files,
	content,
	settings,
	backups,
	access,
	admin,
}
