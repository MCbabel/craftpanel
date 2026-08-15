import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { isApiRequestError } from './client'
import { registrations } from './registration'

const fetchMock = vi.fn<typeof fetch>()

function replies(status: number, body: string | null): typeof fetchMock {
	return fetchMock.mockImplementation(() =>
		Promise.resolve(
			new Response(body, { status, headers: { 'content-type': 'application/json' } }),
		),
	)
}

beforeEach(() => {
	fetchMock.mockReset()
	vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
	vi.unstubAllGlobals()
})

function lastCall(): { url: string; init: RequestInit } {
	const [url, init] = fetchMock.mock.calls.at(-1) as [string, RequestInit]
	return { url, init }
}

describe('the seven calls of section 20', () => {
	it('asks for the options without a body and without a session', async () => {
		replies(
			200,
			'{"registration_enabled":true,"registration_requires_approval":true,"password_reset_enabled":true}',
		)
		const options = await registrations.options()

		const { url, init } = lastCall()
		expect(url).toBe('/api/v1/auth/options')
		expect(init.method).toBe('GET')
		expect(init.body).toBeUndefined()
		expect(options.registration_enabled).toBe(true)
	})

	it('sends the form as JSON', async () => {
		replies(202, '{"status":"check_your_email"}')
		const answer = await registrations.register({
			username: 'max',
			email: 'max@example.test',
			password: 'a-good-password',
		})

		const { url, init } = lastCall()
		expect(url).toBe('/api/v1/auth/register')
		expect(init.method).toBe('POST')
		expect(init.credentials).toBe('same-origin')
		expect(JSON.parse(String(init.body))).toEqual({
			username: 'max',
			email: 'max@example.test',
			password: 'a-good-password',
		})
		expect(answer.status).toBe('check_your_email')
	})

	it('carries the token in the body and never in the address', async () => {
		replies(200, '{"state":"awaiting_approval"}')
		await registrations.verifyEmail('secret-token')

		const { url, init } = lastCall()
		expect(url).toBe('/api/v1/auth/verify-email')
		expect(url).not.toContain('secret-token')
		expect(JSON.parse(String(init.body))).toEqual({ token: 'secret-token' })
	})

	it('sends only the address when asking again', async () => {
		replies(202, '{"status":"check_your_email"}')
		await registrations.resendVerification('max@example.test')

		const { url, init } = lastCall()
		expect(url).toBe('/api/v1/auth/verify-email/resend')
		expect(JSON.parse(String(init.body))).toEqual({ email: 'max@example.test' })
	})

	it('hangs the paging of the queue on as a query', async () => {
		replies(200, '{"registrations":[],"total":0}')
		await registrations.queue({ limit: 25, offset: 50 })

		const { url } = lastCall()
		expect(url).toBe('/api/v1/admin/registrations?limit=25&offset=50')
	})

	it('leaves the query out when nothing is paged', async () => {
		replies(200, '{"registrations":[],"total":0}')
		await registrations.queue()
		expect(lastCall().url).toBe('/api/v1/admin/registrations')
	})

	it('puts the id into the path, encoded', async () => {
		replies(201, '{"id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","username":"max"}')
		await registrations.approve('01ARZ3NDEKTSV4RRFFQ69G5FAV')
		expect(lastCall().url).toBe(
			'/api/v1/admin/registrations/01ARZ3NDEKTSV4RRFFQ69G5FAV/approve',
		)

		replies(204, null)
		await registrations.reject('a/../b')
		expect(lastCall().url).toBe('/api/v1/admin/registrations/a%2F..%2Fb/reject')
	})

	it('sends an empty reason as null', async () => {
		replies(204, null)
		await registrations.reject('01ARZ3NDEKTSV4RRFFQ69G5FAV')
		expect(JSON.parse(String(lastCall().init.body))).toEqual({ reason: null })

		replies(204, null)
		await registrations.reject('01ARZ3NDEKTSV4RRFFQ69G5FAV', '   ')
		expect(JSON.parse(String(lastCall().init.body))).toEqual({ reason: null })

		replies(204, null)
		await registrations.reject('01ARZ3NDEKTSV4RRFFQ69G5FAV', 'spam')
		expect(JSON.parse(String(lastCall().init.body))).toEqual({ reason: 'spam' })
	})
})

describe('the errors from 1.7', () => {
	it('arrive as a code, not as a status line', async () => {
		replies(409, '{"error":"username_taken","message":"max is taken"}')

		await expect(
			registrations.register({ username: 'max', email: 'max@example.test', password: 'x'.repeat(10) }),
		).rejects.toSatisfy((error: unknown) => {
			expect(isApiRequestError(error)).toBe(true)
			if (!isApiRequestError(error)) return false
			expect(error.code).toBe('username_taken')
			expect(error.status).toBe(409)
			return true
		})
	})

	it('turns a network that failed into a code of its own', async () => {
		fetchMock.mockImplementation(() => Promise.reject(new TypeError('offline')))

		await expect(registrations.options()).rejects.toSatisfy((error: unknown) => {
			if (!isApiRequestError(error)) return false
			expect(error.code).toBe('network_unreachable')
			return true
		})
	})

	it('handles plain text from an unknown path without stumbling', async () => {
		fetchMock.mockImplementation(() =>
			Promise.resolve(new Response('Not Found', { status: 404 })),
		)

		await expect(registrations.verifyEmail('abc')).rejects.toSatisfy((error: unknown) => {
			if (!isApiRequestError(error)) return false
			expect(error.code).toBe('http_404')
			return true
		})
	})
})
