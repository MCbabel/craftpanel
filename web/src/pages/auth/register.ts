import type { AuthOptions, VerifiedState } from '@/api/registration'

export const NAME_LENGTH = { min: 3, max: 39 } as const
export const PASSWORD_MIN_LENGTH = 10

export type NameProblem = 'empty' | 'too-short' | 'too-long' | 'bad-characters' | null

export function nameProblem(name: string): NameProblem {
	const text = name.trim()
	if (text === '') return 'empty'
	if (text.length < NAME_LENGTH.min) return 'too-short'
	if (text.length > NAME_LENGTH.max) return 'too-long'
	return /^[a-z0-9_-]+$/.test(text) ? null : 'bad-characters'
}

export function addressLooksRight(address: string): boolean {
	const text = address.trim()
	if (text.length > 254 || /\s/.test(text)) return false
	const parts = text.split('@')
	if (parts.length !== 2) return false
	const [local, domain] = parts as [string, string]
	return (
		local.length > 0 &&
		local.length <= 64 &&
		domain.includes('.') &&
		!domain.startsWith('.') &&
		!domain.endsWith('.') &&
		!domain.includes('..')
	)
}

export function passwordLongEnough(password: string): boolean {
	return [...password].length >= PASSWORD_MIN_LENGTH
}

export function formReady(fields: { username: string; email: string; password: string }): boolean {
	return (
		nameProblem(fields.username) === null &&
		addressLooksRight(fields.email) &&
		passwordLongEnough(fields.password)
	)
}

export type VerifyOutcome = 'done' | 'waiting' | 'expired' | 'unknown' | 'closed' | 'failed'

export function outcomeOf(state: VerifiedState): VerifyOutcome {
	return state === 'active' ? 'done' : 'waiting'
}

export function outcomeOfError(code: string): VerifyOutcome {
	switch (code) {
		case 'token_expired':
			return 'expired'
		case 'invalid_token':
			return 'unknown'
		case 'registration_disabled':
			return 'closed'
		default:
			return 'failed'
	}
}

export function canAskForANewMail(outcome: VerifyOutcome): boolean {
	return outcome === 'expired'
}

export function tokenFromLocation(location: { hash?: string; search?: string }): string | null {
	const fragment = (location.hash ?? '').replace(/^#/, '').trim()
	if (fragment !== '') {
		const inside = new URLSearchParams(fragment).get('token')
		return (inside ?? fragment).trim() || null
	}
	const query = new URLSearchParams(location.search ?? '').get('token')
	return query?.trim() || null
}

export function signUpOpen(options: AuthOptions | null): boolean {
	return options?.registration_enabled === true
}

export function approvalFollows(options: AuthOptions | null): boolean {
	return options?.registration_requires_approval === true
}

export type SignInBlock = 'email_unverified' | 'approval_pending' | null

export function signInBlock(code: string): SignInBlock {
	return code === 'email_unverified' || code === 'approval_pending' ? code : null
}
