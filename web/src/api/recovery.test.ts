import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { isApiRequestError } from './client'
import { recovery } from './recovery'

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

describe('the four calls of section 21', () => {
	it('asks for a link with the address in the body', async () => {
		replies(202, null)
		await recovery.request('max@example.test')

		const { url, init } = lastCall()
		expect(url).toBe('/api/v1/auth/password-reset')
		expect(init.method).toBe('POST')
		expect(init.credentials).toBe('same-origin')
		expect(JSON.parse(String(init.body))).toEqual({ email: 'max@example.test' })
	})

	it('carries the token in the body in both token calls', async () => {
		replies(200, '{"username":"max"}')
		const whose = await recovery.whose('secret-token')
		expect(whose.username).toBe('max')

		let call = lastCall()
		expect(call.url).toBe('/api/v1/auth/password-reset/verify')
		expect(call.url).not.toContain('secret-token')
		expect(JSON.parse(String(call.init.body))).toEqual({ token: 'secret-token' })

		replies(204, null)
		await recovery.confirm('secret-token', 'a-good-password')

		call = lastCall()
		expect(call.url).toBe('/api/v1/auth/password-reset/confirm')
		expect(call.url).not.toContain('secret-token')
		expect(JSON.parse(String(call.init.body))).toEqual({
			token: 'secret-token',
			new_password: 'a-good-password',
		})
	})

	it('puts the user id of the admin nudge into the path, encoded', async () => {
		replies(202, null)
		await recovery.sendFor('01ARZ3NDEKTSV4RRFFQ69G5FAV')
		expect(lastCall().url).toBe(
			'/api/v1/admin/users/01ARZ3NDEKTSV4RRFFQ69G5FAV/password-reset',
		)

		replies(202, null)
		await recovery.sendFor('a/../b')
		expect(lastCall().url).toBe('/api/v1/admin/users/a%2F..%2Fb/password-reset')
	})

	it('takes a 202 without a body without reading JSON', async () => {
		fetchMock.mockImplementation(() => Promise.resolve(new Response(null, { status: 202 })))
		await expect(recovery.request('max@example.test')).resolves.toBeUndefined()
	})
})

describe('the errors', () => {
	it('arrive as a code', async () => {
		replies(400, '{"error":"invalid_reset_token","message":"no longer valid"}')

		await expect(recovery.whose('expired')).rejects.toSatisfy((error: unknown) => {
			if (!isApiRequestError(error)) return false
			expect(error.code).toBe('invalid_reset_token')
			expect(error.status).toBe(400)
			return true
		})
	})

	it('tell the brake apart, so the page can say so', async () => {
		replies(429, '{"error":"too_many_attempts","message":"wait"}')

		await expect(recovery.confirm('abc', 'a-good-password')).rejects.toSatisfy(
			(error: unknown) => {
				if (!isApiRequestError(error)) return false
				expect(error.code).toBe('too_many_attempts')
				return true
			},
		)
	})
})
