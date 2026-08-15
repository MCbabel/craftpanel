import { describe, expect, it } from 'vitest'

import type { Registration } from '@/api/registration'
import {
	canApprove,
	fromTheSameAddress,
	inWorkingOrder,
	queueProblem,
	suspicious,
	waitingForApproval,
	without,
} from '@/pages/admin/registrations'

function row(over: Partial<Registration> = {}): Registration {
	return {
		id: '01JEXZ9K2QW8T7VN4M0P3RCB6D',
		username: 'max',
		email: 'max@example.test',
		state: 'email_unverified',
		signup_ip: '198.51.100.7',
		created_at: '2026-08-12T14:03:11Z',
		verified_at: null,
		...over,
	}
}

const confirmed = row({
	id: '01JEXZ9K2QW8T7VN4M0P3RCB7E',
	username: 'anna',
	state: 'awaiting_approval',
	verified_at: '2026-08-12T15:00:00Z',
	created_at: '2026-08-10T09:00:00Z',
})

describe('the order of the queue', () => {
	it('puts the confirmed ones on top, because only those can be approved', () => {
		const sorted = inWorkingOrder([row(), confirmed])
		expect(sorted.map((entry) => entry.username)).toEqual(['anna', 'max'])
	})

	it('sorts the newest to the top within a group', () => {
		const older = row({ id: 'a', username: 'old', created_at: '2026-08-01T00:00:00Z' })
		const newer = row({ id: 'b', username: 'new', created_at: '2026-08-12T00:00:00Z' })
		expect(inWorkingOrder([older, newer]).map((entry) => entry.username)).toEqual(['new', 'old'])
	})

	it('leaves the list it was handed untouched', () => {
		const rows = [row(), confirmed]
		inWorkingOrder(rows)
		expect(rows.map((entry) => entry.username)).toEqual(['max', 'anna'])
	})

	it('counts how many are waiting for a decision', () => {
		expect(waitingForApproval([row(), confirmed])).toBe(1)
		expect(waitingForApproval([row()])).toBe(0)
		expect(waitingForApproval([])).toBe(0)
	})
})

describe('the sender address (20.5)', () => {
	it('counts how many applications come from the same address', () => {
		const rows = [
			row({ id: 'a' }),
			row({ id: 'b', username: 'two' }),
			row({ id: 'c', username: 'three', signup_ip: '203.0.113.9' }),
		]
		expect(fromTheSameAddress(rows, rows[0]!)).toBe(2)
		expect(fromTheSameAddress(rows, rows[2]!)).toBe(1)
	})

	it('counts nothing when no address is stored', () => {
		const anonymous = row({ signup_ip: null })
		expect(fromTheSameAddress([anonymous, row()], anonymous)).toBe(0)
	})

	it('stands out only from the third on: behind NAT, two are nothing special', () => {
		const two = [row({ id: 'a' }), row({ id: 'b' })]
		expect(suspicious(two, two[0]!)).toBe(false)

		const three = [...two, row({ id: 'c' })]
		expect(suspicious(three, three[0]!)).toBe(true)
	})
})

describe('what the buttons may do', () => {
	it('approves a confirmed application only', () => {
		expect(canApprove(confirmed)).toBe(true)
		expect(canApprove(row())).toBe(false)
	})

	it('maps the error codes of 20.6', () => {
		expect(queueProblem('invalid_state')).toBe('invalid_state')
		expect(queueProblem('username_taken')).toBe('username_taken')
		expect(queueProblem('email_taken')).toBe('email_taken')
		expect(queueProblem('registration_not_found')).toBe('registration_not_found')
		expect(queueProblem('internal')).toBe('failed')
	})

	it('takes a decided row out of the list at once', () => {
		const rows = [row({ id: 'a' }), row({ id: 'b' })]
		expect(without(rows, 'a').map((entry) => entry.id)).toEqual(['b'])
		expect(without(rows, 'does-not-exist')).toHaveLength(2)
	})
})
