import { ApiRequestError, buildUrl, type QueryParams } from './client'
import type { PanelUser, Rfc3339, Ulid } from './types'

export type RegistrationState = 'email_unverified' | 'awaiting_approval'
export type VerifiedState = 'active' | 'awaiting_approval'

export interface AuthOptions {
	registration_enabled: boolean
	registration_requires_approval: boolean
	password_reset_enabled: boolean
}

export interface RegisterRequest {
	username: string
	email: string
	password: string
}

export interface RegisterResponse {
	status: 'check_your_email'
}

export interface VerifyEmailResponse {
	state: VerifiedState
}

export interface Registration {
	id: Ulid
	username: string
	email: string
	state: RegistrationState
	signup_ip: string | null
	created_at: Rfc3339
	verified_at: Rfc3339 | null
}

export interface RegistrationList {
	registrations: Registration[]
	total: number
}

export type RegistrationQuery = { limit?: number; offset?: number }

interface Cancellable {
	signal?: AbortSignal
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

function segment(value: string): string {
	return encodeURIComponent(value)
}

export const registrations = {
	options: (options: Cancellable = {}) => json<AuthOptions>('/auth/options', 'GET', options),

	register: (body: RegisterRequest, options: Cancellable = {}) =>
		json<RegisterResponse>('/auth/register', 'POST', { ...options, body }),

	verifyEmail: (token: string, options: Cancellable = {}) =>
		json<VerifyEmailResponse>('/auth/verify-email', 'POST', { ...options, body: { token } }),

	resendVerification: (email: string, options: Cancellable = {}) =>
		json<RegisterResponse>('/auth/verify-email/resend', 'POST', { ...options, body: { email } }),

	queue: (query: RegistrationQuery = {}, options: Cancellable = {}) =>
		json<RegistrationList>('/admin/registrations', 'GET', { ...options, query }),

	approve: (id: Ulid, options: Cancellable = {}) =>
		json<PanelUser>(`/admin/registrations/${segment(id)}/approve`, 'POST', options),

	reject: async (id: Ulid, reason?: string, options: Cancellable = {}): Promise<void> => {
		await send(`/admin/registrations/${segment(id)}/reject`, 'POST', {
			...options,
			body: { reason: reason?.trim() === '' ? null : (reason ?? null) },
		})
	},
}
