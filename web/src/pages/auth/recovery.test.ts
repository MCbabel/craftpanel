import { describe, expect, it } from 'vitest'

import {
	confirmProblem,
	linkStateOfError,
	newPasswordReady,
	RESEND_AFTER_SECONDS,
	secondsLeft,
	tokenFromLocation,
} from '@/pages/auth/recovery'

describe('the token of the reset link', () => {
	it('stands in the query, the way 21.5 sends it', () => {
		expect(tokenFromLocation({ search: '?token=abc123' })).toBe('abc123')
		expect(tokenFromLocation({ search: '?redirect=/&token=abc123' })).toBe('abc123')
	})

	it('is read out of a fragment as well, so a link typed by hand does not lead nowhere', () => {
		expect(tokenFromLocation({ hash: '#abc123' })).toBe('abc123')
		expect(tokenFromLocation({ hash: '#token=abc123' })).toBe('abc123')
	})

	it('is null when there is none', () => {
		expect(tokenFromLocation({})).toBeNull()
		expect(tokenFromLocation({ search: '?token=' })).toBeNull()
		expect(tokenFromLocation({ search: '?other=1', hash: '#' })).toBeNull()
	})
})

describe('a link that no longer holds', () => {
	it('looks the same in all three cases', () => {
		expect(linkStateOfError('invalid_reset_token')).toBe('dead')
		expect(linkStateOfError('too_many_attempts')).toBe('dead')
		expect(linkStateOfError('internal')).toBe('dead')
	})

	it('tells apart only the network that failed, because nothing is decided there', () => {
		expect(linkStateOfError('network_unreachable')).toBe('unreachable')
	})
})

describe('the new password', () => {
	it('needs two matching entries and the minimum length', () => {
		expect(newPasswordReady('a-good-password', 'a-good-password', 10)).toBe(true)
		expect(newPasswordReady('a-good-password', 'a-good-passwor', 10)).toBe(false)
		expect(newPasswordReady('short', 'short', 10)).toBe(false)
		expect(newPasswordReady('', '', 10)).toBe(false)
	})

	it('counts characters, not bytes', () => {
		expect(newPasswordReady('äöüäöüäöüä', 'äöüäöüäöüä', 10)).toBe(true)
		expect(newPasswordReady('äöüäöüäöü', 'äöüäöüäöü', 10)).toBe(false)
	})

	it('maps the error codes of 21.3', () => {
		expect(confirmProblem('weak_password')).toBe('weak_password')
		expect(confirmProblem('invalid_reset_token')).toBe('invalid_reset_token')
		expect(confirmProblem('too_many_attempts')).toBe('too_many_attempts')
		expect(confirmProblem('internal')).toBe('failed')
	})
})

describe('the time left on the button', () => {
	it('runs from sixty down to zero and stays there', () => {
		const asked = 1_000_000
		expect(secondsLeft(asked, asked)).toBe(RESEND_AFTER_SECONDS)
		expect(secondsLeft(asked, asked + 30_000)).toBe(30)
		expect(secondsLeft(asked, asked + 60_000)).toBe(0)
		expect(secondsLeft(asked, asked + 600_000)).toBe(0)
	})

	it('is zero while nothing has been sent', () => {
		expect(secondsLeft(null, 1_000_000)).toBe(0)
	})
})
