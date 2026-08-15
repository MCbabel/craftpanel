export const LINK_MINUTES = 30
export const RESEND_AFTER_SECONDS = 60

export type LinkState = 'checking' | 'ready' | 'dead' | 'unreachable'

export function linkStateOfError(code: string): LinkState {
	return code === 'network_unreachable' ? 'unreachable' : 'dead'
}

export function tokenFromLocation(location: { search?: string; hash?: string }): string | null {
	const query = new URLSearchParams(location.search ?? '').get('token')
	if (query?.trim()) return query.trim()
	const fragment = (location.hash ?? '').replace(/^#/, '').trim()
	if (fragment === '') return null
	const inside = new URLSearchParams(fragment).get('token')
	return (inside ?? fragment).trim() || null
}

export function newPasswordReady(chosen: string, repeated: string, minLength: number): boolean {
	return [...chosen].length >= minLength && chosen === repeated
}

export type ConfirmProblem = 'weak_password' | 'invalid_reset_token' | 'too_many_attempts' | 'failed'

export function confirmProblem(code: string): ConfirmProblem {
	switch (code) {
		case 'weak_password':
		case 'invalid_reset_token':
		case 'too_many_attempts':
			return code
		default:
			return 'failed'
	}
}

export function secondsLeft(askedAt: number | null, now: number): number {
	if (askedAt === null) return 0
	const passed = Math.floor((now - askedAt) / 1000)
	return Math.max(0, RESEND_AFTER_SECONDS - passed)
}
