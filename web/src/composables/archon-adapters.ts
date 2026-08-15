import type { AbstractModrinthClient, Archon } from '@modrinth/api-client'
import { injectModrinthClient, provideModrinthClient } from '@modrinth/ui'

import { api, type Backup, type BackupOperation, isApiRequestError, type Ulid } from '@/api'

function rethrow(cause: unknown): never {
	if (isApiRequestError(cause)) throw new Error(`${cause.status} ${cause.code}: ${cause.message}`)
	throw cause
}

function toQueueOperation(operation: BackupOperation): Archon.BackupsQueue.v1.BackupQueueOperation {
	return { ...operation, operation_id: operation.operation_id as unknown as number }
}

function toQueueBackup(backup: Backup): Archon.BackupsQueue.v1.BackupQueueBackup {
	return { ...backup, history: backup.history.map(toQueueOperation) }
}

function buildModules(serverId: Ulid) {
	const backups_queue_v1 = {
		list: async (): Promise<Archon.BackupsQueue.v1.BackupsQueueResponse> => {
			const page = await api.backups.list(serverId).catch(rethrow)
			return {
				active_operations: page.active_operations.map((operation) => ({
					...operation,
					operation_id: operation.operation_id as unknown as number,
				})),
				backups: page.backups.map(toQueueBackup),
			}
		},
		create: (_serverId: string, _worldId: string, request: { name: string }) =>
			api.backups.create(serverId, request).catch(rethrow),
		delete: (_serverId: string, _worldId: string, backupId: string) =>
			api.backups.remove(serverId, backupId).catch(rethrow),
		deleteMany: (_serverId: string, _worldId: string, backupIds: string[]) =>
			api.backups.bulkDelete(serverId, { backup_ids: backupIds }).catch(rethrow),
		restore: (_serverId: string, _worldId: string, backupId: string, request: { name: string }) =>
			api.backups.restore(serverId, backupId, request).catch(rethrow),
		retry: (_serverId: string, _worldId: string, backupId: string) =>
			api.backups.retry(serverId, backupId).catch(rethrow),
		ackCreate: (_serverId: string, _worldId: string, operationId: string) =>
			api.operations.dismiss(serverId, operationId).catch(rethrow),
		ackRestore: (_serverId: string, _worldId: string, operationId: string) =>
			api.operations.dismiss(serverId, operationId).catch(rethrow),
		cancelCreate: (_serverId: string, _worldId: string, operationId: string) =>
			api.operations.cancel(serverId, operationId).catch(rethrow),
		cancelRestore: (_serverId: string, _worldId: string, operationId: string) =>
			api.operations.cancel(serverId, operationId).catch(rethrow),
	}

	const backups_v1 = {
		rename: (_serverId: string, _worldId: string, backupId: string, request: { name: string }) =>
			api.backups.rename(serverId, backupId, request).catch(rethrow),
		delete: (_serverId: string, _worldId: string, backupId: string) =>
			api.backups.remove(serverId, backupId).catch(rethrow),
	}

	const properties_v1 = {
		getProperties: () => api.settings.properties(serverId).catch(rethrow),
		patchProperties: (
			_serverId: string,
			_worldId: string,
			patch: Archon.Content.v1.PatchPropertiesFields,
		) => api.settings.patchProperties(serverId, patch).catch(rethrow),
	}

	return { backups_queue_v1, backups_v1, properties_v1 }
}

export function provideArchonAdapters(serverId: Ulid): void {
	const real = injectModrinthClient()
	const shim: AbstractModrinthClient = Object.create(real)
	Object.defineProperty(shim, 'archon', {
		value: { ...real.archon, ...buildModules(serverId) },
		enumerable: true,
	})
	provideModrinthClient(shim)
}
