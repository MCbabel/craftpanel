import { describe, expect, it } from 'vitest'

import type { AuthOptions } from '@/api/registration'
import {
	addressLooksRight,
	approvalFollows,
	canAskForANewMail,
	formReady,
	nameProblem,
	outcomeOf,
	outcomeOfError,
	passwordLongEnough,
	signInBlock,
	signUpOpen,
	tokenFromLocation,
} from '@/pages/auth/register'

function options(over: Partial<AuthOptions> = {}): AuthOptions {
	return {
		registration_enabled: false,
		registration_requires_approval: true,
		password_reset_enabled: false,
		...over,
	}
}

describe('the rules of the fields', () => {
	it('takes the same names as 12.3 and rejects the same ones', () => {
		expect(nameProblem('max')).toBeNull()
		expect(nameProblem('a-b_c9')).toBeNull()
		expect(nameProblem('a'.repeat(39))).toBeNull()

		expect(nameProblem('')).toBe('empty')
		expect(nameProblem('ma')).toBe('too-short')
		expect(nameProblem('a'.repeat(40))).toBe('too-long')
		expect(nameProblem('Max')).toBe('bad-characters')
		expect(nameProblem('max morgan')).toBe('bad-characters')
		expect(nameProblem('max.morgan')).toBe('bad-characters')
	})

	it('checks the address as thinly as 20.10 and no more strictly', () => {
		expect(addressLooksRight('max@example.test')).toBe(true)
		expect(addressLooksRight('max.morgan+panel@example.co.uk')).toBe(true)

		expect(addressLooksRight('max')).toBe(false)
		expect(addressLooksRight('max@example')).toBe(false)
		expect(addressLooksRight('max@@example.test')).toBe(false)
		expect(addressLooksRight('max @example.test')).toBe(false)
		expect(addressLooksRight('max@.example.test')).toBe(false)
		expect(addressLooksRight('max@example..test')).toBe(false)
		expect(addressLooksRight(`${'a'.repeat(65)}@example.test`)).toBe(false)
	})

	it('counts the password in characters, not in bytes', () => {
		expect(passwordLongEnough('äöüäöüäöüä')).toBe(true)
		expect(passwordLongEnough('äöüäöüäöü')).toBe(false)
		expect(passwordLongEnough('1234567890')).toBe(true)
		expect(passwordLongEnough('123456789')).toBe(false)
	})

	it('releases the button only once all three fields are good', () => {
		const good = { username: 'max', email: 'max@example.test', password: 'a-good-password' }
		expect(formReady(good)).toBe(true)
		expect(formReady({ ...good, username: 'Max' })).toBe(false)
		expect(formReady({ ...good, email: 'max' })).toBe(false)
		expect(formReady({ ...good, password: 'short' })).toBe(false)
	})
})

describe('the token from the address bar', () => {
	it('reads it out of the fragment, the way 20.9 sends it', () => {
		expect(tokenFromLocation({ hash: '#abc123' })).toBe('abc123')
		expect(tokenFromLocation({ hash: '#token=abc123' })).toBe('abc123')
	})

	it('takes the query as well, so a swallowed fragment is no dead end', () => {
		expect(tokenFromLocation({ search: '?token=abc123' })).toBe('abc123')
		expect(tokenFromLocation({ hash: '', search: '?token=abc123' })).toBe('abc123')
	})

	it('returns null when there is nothing there', () => {
		expect(tokenFromLocation({})).toBeNull()
		expect(tokenFromLocation({ hash: '#', search: '' })).toBeNull()
		expect(tokenFromLocation({ search: '?other=1' })).toBeNull()
	})

	it('prefers the fragment, because it reaches no server', () => {
		expect(tokenFromLocation({ hash: '#fromthefragment', search: '?token=fromthequery' })).toBe(
			'fromthefragment',
		)
	})
})

describe('the three outcomes of the confirmation (20.3)', () => {
	it('turns the state into an outcome', () => {
		expect(outcomeOf('active')).toBe('done')
		expect(outcomeOf('awaiting_approval')).toBe('waiting')
	})

	it('and every error code into exactly one', () => {
		expect(outcomeOfError('token_expired')).toBe('expired')
		expect(outcomeOfError('invalid_token')).toBe('unknown')
		expect(outcomeOfError('registration_disabled')).toBe('closed')
		expect(outcomeOfError('internal')).toBe('failed')
	})

	it('offers a new mail only when the old one has expired', () => {
		expect(canAskForANewMail('expired')).toBe(true)
		expect(canAskForANewMail('unknown')).toBe(false)
		expect(canAskForANewMail('done')).toBe(false)
		expect(canAskForANewMail('waiting')).toBe(false)
	})
})

describe('what the sign-in page learns from 20.1', () => {
	it('shows no form while sign-up is closed', () => {
		expect(signUpOpen(null)).toBe(false)
		expect(signUpOpen(options())).toBe(false)
		expect(signUpOpen(options({ registration_enabled: true }))).toBe(true)
	})

	it('says before sending whether an approval follows', () => {
		expect(approvalFollows(options({ registration_requires_approval: true }))).toBe(true)
		expect(approvalFollows(options({ registration_requires_approval: false }))).toBe(false)
		expect(approvalFollows(null)).toBe(false)
	})
})

describe('the two new states of signing in (20.8)', () => {
	it('knows them and leaves everything else alone', () => {
		expect(signInBlock('email_unverified')).toBe('email_unverified')
		expect(signInBlock('approval_pending')).toBe('approval_pending')
		expect(signInBlock('invalid_credentials')).toBeNull()
		expect(signInBlock('too_many_attempts')).toBeNull()
	})
})
