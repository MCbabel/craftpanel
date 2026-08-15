import { ApiRequestError, buildUrl, isApiRequestError, type QueryParams } from './client'
import type { BackupLocation, DriveFileState, Rfc3339, Ulid } from './types'

export type DriveAccountState = 'connected' | 'revoked' | 'error'
export type DriveLinkState = 'waiting' | 'accepted' | 'denied' | 'expired'
export type BackupTargetPolicy = 'user_choice' | 'drive_only' | 'local_only'
export type DriveFileDisposal = 'delete' | 'keep'
export type BackupTargetReason = 'ok' | 'not_configured' | 'not_connected' | 'policy'

export interface DriveLink {
	user_code: string
	verification_url: string
	state: DriveLinkState
	started_at: Rfc3339
	expires_at: Rfc3339
	interval: number
}

export interface DriveStatus {
	panel_configured: boolean
	configured: boolean
	state: DriveAccountState | null
	google_name: string | null
	google_email: string | null
	folder_name: string
	storage_limit_bytes: number | null
	storage_usage_bytes: number | null
	link: DriveLink | null
	last_error: string | null
	checked_at: Rfc3339 | null
}

export interface DriveOverview {
	user_id: Ulid
	username: string
	state: DriveAccountState | null
	google_email: string | null
	storage_limit_bytes: number | null
	storage_usage_bytes: number | null
	backups: number
	backup_bytes: number
	last_error: string | null
	checked_at: Rfc3339 | null
}

export interface DriveAdminOverview {
	configured: boolean
	client_id: string | null
	target_policy: BackupTargetPolicy
	folder_name: string
	accounts: DriveOverview[]
}

export interface UpdateDriveSettingsRequest {
	client_id: string | null
	client_secret?: string | null
	target_policy: BackupTargetPolicy
	folder_name: string
}

export interface BackupTarget {
	target: BackupLocation
	effective_target: BackupLocation
	policy: BackupTargetPolicy
	reason: BackupTargetReason
}

interface Cancellable {
	signal?: AbortSignal
}

function segment(value: string): string {
	return encodeURIComponent(value)
}

function failure(status: number, raw: string): ApiRequestError {
	let code = status === 401 ? 'unauthenticated' : `http_${status}`
	let message = `Request failed with status ${status}`
	try {
		const parsed: unknown = JSON.parse(raw)
		if (typeof parsed === 'object' && parsed !== null) {
			const envelope = parsed as { error?: unknown; message?: unknown }
			if (typeof envelope.error === 'string') code = envelope.error
			if (typeof envelope.message === 'string') message = envelope.message
		}
	} catch {
	}
	return new ApiRequestError(status, code, message)
}

async function send(
	path: string,
	method: string,
	options: Cancellable & { query?: QueryParams; body?: unknown } = {},
): Promise<Response> {
	let response: Response
	try {
		response = await fetch(buildUrl(path, options.query), {
			method,
			credentials: 'same-origin',
			signal: options.signal,
			...(options.body === undefined
				? {}
				: {
						headers: { 'Content-Type': 'application/json' },
						body: JSON.stringify(options.body),
					}),
		})
	} catch (cause) {
		if (options.signal?.aborted) throw cause
		throw new ApiRequestError(0, 'network_unreachable', 'The panel could not be reached.')
	}
	if (response.ok) return response
	throw failure(response.status, await response.text().catch(() => ''))
}

async function json<T>(
	path: string,
	method: string,
	options: Cancellable & { query?: QueryParams; body?: unknown } = {},
): Promise<T> {
	return (await (await send(path, method, options)).json()) as T
}

async function nothing(
	path: string,
	method: string,
	options: Cancellable & { query?: QueryParams } = {},
): Promise<void> {
	await send(path, method, options)
}

function targetPath(serverId: Ulid): string {
	return `/servers/${segment(serverId)}/backups/target`
}

export const drive = {
	status: (options: Cancellable = {}) => json<DriveStatus>('/drive', 'GET', options),

	startLink: (options: Cancellable = {}) => json<DriveLink>('/drive/link', 'POST', options),

	link: (options: Cancellable = {}) => json<DriveLink>('/drive/link', 'GET', options),

	cancelLink: (options: Cancellable = {}) => nothing('/drive/link', 'DELETE', options),

	check: (options: Cancellable = {}) => json<DriveStatus>('/drive/check', 'POST', options),

	disconnect: (files?: DriveFileDisposal, options: Cancellable = {}) =>
		nothing('/drive', 'DELETE', { ...options, query: files ? { files } : undefined }),

	overview: (options: Cancellable = {}) =>
		json<DriveAdminOverview>('/admin/drive', 'GET', options),

	save: (body: UpdateDriveSettingsRequest, options: Cancellable = {}) =>
		json<DriveAdminOverview>('/admin/drive', 'PUT', { ...options, body }),

	forgetCredentials: (options: Cancellable = {}) =>
		nothing('/admin/drive/credentials', 'DELETE', options),

	disconnectUser: (userId: Ulid, options: Cancellable = {}) =>
		nothing(`/admin/drive/${segment(userId)}`, 'DELETE', options),

	target: (serverId: Ulid, options: Cancellable = {}) =>
		json<BackupTarget>(targetPath(serverId), 'GET', options),

	setTarget: (serverId: Ulid, target: BackupLocation, options: Cancellable = {}) =>
		json<BackupTarget>(targetPath(serverId), 'PUT', { ...options, body: { target } }),
}

export type DriveLinkPhase = 'waiting' | 'accepted' | 'denied' | 'expired'

export function linkPhase(link: DriveLink, now: number = Date.now()): DriveLinkPhase {
	if (link.state === 'accepted') return 'accepted'
	if (link.state === 'denied') return 'denied'
	if (link.state === 'expired') return 'expired'
	const until = Date.parse(link.expires_at)
	return Number.isFinite(until) && until <= now ? 'expired' : 'waiting'
}

export function noLinkOpen(error: unknown): boolean {
	return isApiRequestError(error) && error.status === 404
}

export function statusPollMs(status: DriveStatus | null, now: number = Date.now()): number | null {
	if (status === null || !status.panel_configured) return null
	if (status.link !== null && linkPhase(status.link, now) === 'waiting') {
		return Math.max(1, status.link.interval) * 1000
	}
	return status.configured ? 30_000 : null
}

export function storageLeft(status: DriveStatus | DriveOverview): number | null {
	if (status.storage_limit_bytes === null) return null
	return Math.max(0, status.storage_limit_bytes - (status.storage_usage_bytes ?? 0))
}

export function storageShare(status: DriveStatus | DriveOverview): number | null {
	const limit = status.storage_limit_bytes
	if (limit === null || limit <= 0) return null
	return Math.min(1, Math.max(0, (status.storage_usage_bytes ?? 0) / limit))
}
