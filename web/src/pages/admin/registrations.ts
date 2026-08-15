import type { Registration } from '@/api/registration'

export function inWorkingOrder(rows: readonly Registration[]): Registration[] {
	const rank = (row: Registration) => (row.state === 'awaiting_approval' ? 0 : 1)
	return [...rows].sort(
		(left, right) => rank(left) - rank(right) || right.created_at.localeCompare(left.created_at),
	)
}

export function waitingForApproval(rows: readonly Registration[]): number {
	return rows.filter((row) => row.state === 'awaiting_approval').length
}

export function fromTheSameAddress(rows: readonly Registration[], row: Registration): number {
	if (row.signup_ip === null) return 0
	return rows.filter((other) => other.signup_ip === row.signup_ip).length
}

export function suspicious(rows: readonly Registration[], row: Registration): boolean {
	return fromTheSameAddress(rows, row) > 2
}

export function canApprove(row: Registration): boolean {
	return row.state === 'awaiting_approval'
}

export type QueueProblem =
	| 'registration_not_found'
	| 'invalid_state'
	| 'username_taken'
	| 'email_taken'
	| 'failed'

export function queueProblem(code: string): QueueProblem {
	switch (code) {
		case 'registration_not_found':
		case 'invalid_state':
		case 'username_taken':
		case 'email_taken':
			return code
		default:
			return 'failed'
	}
}

export function without(rows: readonly Registration[], id: string): Registration[] {
	return rows.filter((row) => row.id !== id)
}
