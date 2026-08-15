import type { Operation, ServerStatus } from '@/api'

export type ServerNotice =
	| { kind: 'delete-failed'; reason: string }
	| { kind: 'deleting' }
	| { kind: 'broken' }
	| { kind: 'setup-pending' }
	| null

export function serverNotice(
	status: ServerStatus,
	setupPending: boolean,
	operations: readonly Operation[],
): ServerNotice {
	if (status === 'deleting') {
		const running = operations.some(
			(operation) =>
				operation.kind === 'server_delete' &&
				(operation.state === 'queued' || operation.state === 'ongoing'),
		)
		const failed = operations.find(
			(operation) => operation.kind === 'server_delete' && operation.state === 'failed',
		)
		if (!running && failed) return { kind: 'delete-failed', reason: failed.error?.message ?? '' }
		return { kind: 'deleting' }
	}
	if (status === 'broken') return { kind: 'broken' }
	if (setupPending) return { kind: 'setup-pending' }
	return null
}
