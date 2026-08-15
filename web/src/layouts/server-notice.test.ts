import { describe, expect, it } from 'vitest'

import type { Operation } from '@/api'

import { serverNotice } from './server-notice'

function operation(fields: Partial<Operation>): Operation {
	return {
		id: '01M01X3W9JJY6J5FFYSKD9VTAA',
		server_id: '01M01X3W9MPPQNKNJ1WY3BKVCS',
		kind: 'server_delete',
		state: 'ongoing',
		phase: null,
		progress: 0,
		message: null,
		src: null,
		bytes_processed: null,
		files_processed: null,
		current_file: null,
		error: null,
		cancellable: false,
		target_id: null,
		started_by: null,
		created_at: '2026-08-15T10:00:00Z',
		started_at: null,
		finished_at: null,
		dismissed_at: null,
		...fields,
	}
}

describe('serverNotice', () => {
	it('reports the deletion that is running', () => {
		expect(serverNotice('deleting', false, [operation({})])).toEqual({ kind: 'deleting' })
	})

	it('names the reason when the deletion failed', () => {
		const failed = operation({
			state: 'failed',
			error: {
				code: 'permission_denied',
				message: 'Permission denied (os error 13)',
				step: 'filesystem',
			},
		})
		expect(serverNotice('deleting', false, [failed])).toEqual({
			kind: 'delete-failed',
			reason: 'Permission denied (os error 13)',
		})
	})

	it('names the reason even when the run was clicked away', () => {
		const wiped = operation({
			state: 'failed',
			error: { code: 'permission_denied', message: 'Permission denied', step: 'filesystem' },
			finished_at: '2026-08-15T10:01:00Z',
			dismissed_at: '2026-08-15T10:02:00Z',
		})
		expect(serverNotice('deleting', false, [wiped])).toEqual({
			kind: 'delete-failed',
			reason: 'Permission denied',
		})
	})

	it('prefers the running second attempt to the failed first one', () => {
		const first = operation({
			id: '01M01X3W9JJY6J5FFYSKD9VTAA',
			state: 'failed',
			error: { code: 'permission_denied', message: 'Permission denied', step: 'filesystem' },
		})
		const second = operation({ id: '01M01X4ZZZJY6J5FFYSKD9VTBB', state: 'queued' })
		expect(serverNotice('deleting', false, [second, first])).toEqual({ kind: 'deleting' })
	})

	it('leaves a failed run of another kind alone', () => {
		const failed = operation({ kind: 'server_create', state: 'failed' })
		expect(serverNotice('deleting', false, [failed])).toEqual({ kind: 'deleting' })
	})

	it('reports the broken server and the setup still open', () => {
		expect(serverNotice('broken', false, [])).toEqual({ kind: 'broken' })
		expect(serverNotice('available', true, [])).toEqual({ kind: 'setup-pending' })
		expect(serverNotice('available', false, [])).toBeNull()
	})
})
