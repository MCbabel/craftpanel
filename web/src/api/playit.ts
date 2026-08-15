import {
	ApiRequestError,
	buildUrl,
	hasErrorCode,
	isApiRequestError,
	type QueryParams,
} from './client'
import type { Rfc3339, Ulid } from './types'

export type PlayitAgentState = 'absent' | 'starting' | 'running' | 'failed'
export type PlayitBinaryState = 'absent' | 'fetching' | 'ready' | 'failed'
export type PlayitAccountStatus = 'guest' | 'email_not_verified' | 'verified'
export type PlayitClaimState = 'waiting_for_visit' | 'waiting_for_user' | 'accepted' | 'rejected'
export type PlayitTunnelState = 'none' | 'pending' | 'online' | 'offline' | 'missing' | 'failed'
export type PlayitAddressKind = 'auto' | 'ip4' | 'ip6' | 'addr4' | 'addr6' | 'domain'
export type PlayitTunnelDisposal = 'delete' | 'keep'

export interface PlayitClaim {
	code: string
	url: string
	state: PlayitClaimState
	started_at: Rfc3339
	expires_at: Rfc3339
}

export interface PlayitAgent {
	state: PlayitAgentState
	version: string | null
	detail: string | null
}

export interface PlayitPorts {
	used: number
	limit: number
	for_others: number
}

export interface PlayitStatus {
	configured: boolean
	agent_id: string | null
	account_status: PlayitAccountStatus | null
	is_self_managed: boolean
	has_premium: boolean
	agent: PlayitAgent
	binary: { state: PlayitBinaryState; version: string | null; arch: string; detail: string | null }
	ports: PlayitPorts
	claim: PlayitClaim | null
	last_error: string | null
	checked_at: Rfc3339 | null
}

export interface PlayitOverview {
	user_id: Ulid
	username: string | null
	configured: boolean
	account_status: PlayitAccountStatus | null
	is_self_managed: boolean
	has_premium: boolean
	agent: PlayitAgent
	ports: PlayitPorts
	last_error: string | null
	checked_at: Rfc3339 | null
}

export interface PlayitAddress {
	address: string
	kind: PlayitAddressKind
}

export interface ServerTunnel {
	state: PlayitTunnelState
	addresses: PlayitAddress[]
	local_port: number | null
	detail: string | null
	created_at: Rfc3339 | null
	checked_at: Rfc3339 | null
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
	options: Cancellable & { query?: QueryParams } = {},
): Promise<Response> {
	let response: Response
	try {
		response = await fetch(buildUrl(path, options.query), {
			method,
			credentials: 'same-origin',
			signal: options.signal,
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
	options: Cancellable & { query?: QueryParams } = {},
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

function tunnelPath(serverId: Ulid): string {
	return `/servers/${segment(serverId)}/playit`
}

export const playit = {
	status: (options: Cancellable = {}) => json<PlayitStatus>('/playit', 'GET', options),

	startClaim: (options: Cancellable = {}) => json<PlayitClaim>('/playit/claim', 'POST', options),

	claim: (options: Cancellable = {}) => json<PlayitClaim>('/playit/claim', 'GET', options),

	cancelClaim: (options: Cancellable = {}) => nothing('/playit/claim', 'DELETE', options),

	disconnect: (tunnels?: PlayitTunnelDisposal, options: Cancellable = {}) =>
		nothing('/playit', 'DELETE', { ...options, query: tunnels ? { tunnels } : undefined }),

	restartAgent: (options: Cancellable = {}) =>
		json<PlayitStatus>('/playit/agent/restart', 'POST', options),

	overview: (options: Cancellable = {}) =>
		json<PlayitOverview[]>('/admin/playit', 'GET', options),

	disconnectUser: (
		userId: Ulid,
		tunnels?: PlayitTunnelDisposal,
		options: Cancellable = {},
	) =>
		nothing(`/admin/playit/${segment(userId)}`, 'DELETE', {
			...options,
			query: tunnels ? { tunnels } : undefined,
		}),

	tunnel: (serverId: Ulid, options: Cancellable = {}) =>
		json<ServerTunnel>(tunnelPath(serverId), 'GET', options),

	createTunnel: (serverId: Ulid, options: Cancellable = {}) =>
		json<ServerTunnel>(tunnelPath(serverId), 'POST', options),

	deleteTunnel: (serverId: Ulid, options: Cancellable = {}) =>
		nothing(tunnelPath(serverId), 'DELETE', options),
}

export function tunnelAddresses(tunnel: ServerTunnel | null): PlayitAddress[] {
	return tunnel?.addresses ?? []
}

export function shareAddress(tunnel: ServerTunnel | null): PlayitAddress | null {
	return tunnelAddresses(tunnel)[0] ?? null
}

export function otherAddresses(tunnel: ServerTunnel | null): PlayitAddress[] {
	return tunnelAddresses(tunnel).slice(1)
}

export function publicAddress(tunnel: ServerTunnel | null): string | null {
	if (tunnel?.state !== 'online') return null
	return shareAddress(tunnel)?.address ?? null
}

export type PlayitClaimPhase = 'waiting' | 'accepted' | 'rejected' | 'expired'

export function claimPhase(claim: PlayitClaim, now: number = Date.now()): PlayitClaimPhase {
	if (claim.state === 'accepted') return 'accepted'
	if (claim.state === 'rejected') return 'rejected'
	return Date.parse(claim.expires_at) <= now ? 'expired' : 'waiting'
}

export function notFound(error: unknown): boolean {
	return isApiRequestError(error) && error.status === 404
}

export function playitAbsent(error: unknown): boolean {
	return notFound(error) && !hasErrorCode(error, 'server_not_found')
}

export function portsLeft(status: PlayitStatus | PlayitOverview): number {
	return Math.max(0, status.ports.limit - status.ports.used)
}

export function statusPollMs(status: PlayitStatus): number {
	const moving = status.agent.state === 'starting' || status.binary.state === 'fetching'
	return moving ? 3_000 : 20_000
}

export function tunnelPollMs(tunnel: ServerTunnel | null, available: boolean): number | null {
	if (!available) return null
	return tunnel?.state === 'pending' ? 2_000 : 30_000
}
