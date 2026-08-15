import { createContext } from '@modrinth/ui'

import type { ServerEventSource, Ulid } from '@/api'
import type { ServerContextHandle } from '@/providers'

export interface ServerPage extends ServerContextHandle {
	serverId: Ulid
	socket: ServerEventSource
}

export const [useServerPage, provideServerPage] = createContext<ServerPage>(
	'ServerShell',
	'serverPage',
)
