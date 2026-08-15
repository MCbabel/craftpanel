import { ApiRequestError, buildUrl, type QueryParams } from './client'
import type { Rfc3339, Ulid } from './types'

export type MailProvider = 'resend'
export type MailState = 'not_configured' | 'configured' | 'file_sink'
export type MailKind =
	| 'verify_email'
	| 'address_already_registered'
	| 'account_awaiting_review'
	| 'account_approved'
	| 'account_rejected'
	| 'reset_password'
	| 'password_changed'
	| 'test'
export type MailDeliveryState = 'queued' | 'sending' | 'sent' | 'failed'

export const MAIL_KINDS: readonly MailKind[] = [
	'verify_email',
	'address_already_registered',
	'account_awaiting_review',
	'account_approved',
	'account_rejected',
	'reset_password',
	'password_changed',
	'test',
]

export interface MailSettings {
	provider: MailProvider
	state: MailState
	key_set_at: Rfc3339 | null
	from_address: string
	from_name: string
	reply_to: string | null
	link_base: string | null
	example_link: string | null
	sink_path: string | null
	daily_limit: number
	sent_today: number
	queued: number
	failed: number
	last_test_at: Rfc3339 | null
	last_error: string | null
	last_error_at: Rfc3339 | null
}

export interface UpdateMailSettingsRequest {
	from_address: string
	from_name: string
	reply_to: string | null
	link_base: string | null
	daily_limit: number
	api_key?: string | null
}

export interface SendTestMailRequest {
	to?: string
}

export interface SendTestMailResponse {
	id: string
	to: string
}

export interface MailOutboxEntry {
	id: Ulid
	kind: MailKind
	to_address: string
	subject: string
	state: MailDeliveryState
	attempts: number
	next_attempt_at: Rfc3339 | null
	provider_id: string | null
	last_error: string | null
	has_content: boolean
	created_at: Rfc3339
	sent_at: Rfc3339 | null
}

export interface MailOutboxList {
	mails: MailOutboxEntry[]
	total: number
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
			headers: options.body === undefined ? undefined : { 'content-type': 'application/json' },
			body: options.body === undefined ? undefined : JSON.stringify(options.body),
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
	options: Cancellable & { query?: QueryParams; body?: unknown } = {},
): Promise<T> {
	return (await (await send(path, method, options)).json()) as T
}

export const mail = {
	settings: (options: Cancellable = {}) => json<MailSettings>('/admin/mail', 'GET', options),

	save: (body: UpdateMailSettingsRequest, options: Cancellable = {}) =>
		json<MailSettings>('/admin/mail', 'PUT', { ...options, body }),

	dropKey: async (options: Cancellable = {}): Promise<void> => {
		await send('/admin/mail/key', 'DELETE', options)
	},

	test: (body: SendTestMailRequest, options: Cancellable = {}) =>
		json<SendTestMailResponse>('/admin/mail/test', 'POST', { ...options, body }),

	outbox: (
		query: { limit?: number; state?: MailDeliveryState } = {},
		options: Cancellable = {},
	) => json<MailOutboxList>('/admin/mail/outbox', 'GET', { ...options, query }),

	retry: async (id: Ulid, options: Cancellable = {}): Promise<void> => {
		await send(`/admin/mail/outbox/${segment(id)}/retry`, 'POST', options)
	},
}

export function contentUrl(id: Ulid): string {
	return buildUrl(`/admin/mail/outbox/${segment(id)}/content`)
}

export function previewUrl(kind: MailKind): string {
	return buildUrl(`/admin/mail/preview/${segment(kind)}`)
}
