import { ApiRequestError, buildUrl } from './client'
import type { Ulid } from './types'

export interface WhosePasswordResponse {
	username: string
}

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
	options: Cancellable & { body?: unknown } = {},
): Promise<Response> {
	let response: Response
	try {
		response = await fetch(buildUrl(path), {
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

export const recovery = {
	request: async (email: string, options: Cancellable = {}): Promise<void> => {
		await send('/auth/password-reset', 'POST', { ...options, body: { email } })
	},

	whose: async (token: string, options: Cancellable = {}): Promise<WhosePasswordResponse> =>
		(await (
			await send('/auth/password-reset/verify', 'POST', { ...options, body: { token } })
		).json()) as WhosePasswordResponse,

	confirm: async (
		token: string,
		newPassword: string,
		options: Cancellable = {},
	): Promise<void> => {
		await send('/auth/password-reset/confirm', 'POST', {
			...options,
			body: { token, new_password: newPassword },
		})
	},

	sendFor: async (user: Ulid, options: Cancellable = {}): Promise<void> => {
		await send(`/admin/users/${encodeURIComponent(user)}/password-reset`, 'POST', options)
	},
}
