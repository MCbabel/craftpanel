<template>
	<div class="flex flex-col gap-6">
		<Admonition
			v-if="restartRequired"
			type="warning"
			:header="formatMessage(messages.restartHeader)"
			:body="formatMessage(messages.restartBody)"
		/>

		<div class="flex flex-col gap-2.5">
			<span class="text-lg font-semibold text-contrast">
				{{ formatMessage(messages.addressLabel) }}
			</span>
			<CopyCode :text="address" />
			<span>
				{{ formatMessage(tunnelHolds ? messages.addressHelpTunnelled : messages.addressHelp) }}
			</span>
		</div>

		<div v-if="tunnelHolds" class="flex flex-col gap-2.5">
			<span class="text-lg font-semibold text-contrast">
				{{ formatMessage(messages.publicTitle) }}
			</span>
			<span>
				{{
					publicAddress
						? formatMessage(messages.publishedAt, { port: published, address: publicAddress })
						: formatMessage(messages.publishing, { port: published })
				}}
			</span>
			<span>{{ formatMessage(messages.onlyThatOnePort) }}</span>
			<span>{{ formatMessage(messages.swapNeedsGiveBack) }}</span>
		</div>

		<div class="flex flex-col gap-2.5">
			<span class="text-lg font-semibold text-contrast">
				{{ formatMessage(messages.allocations) }}
			</span>

			<div class="flex w-full flex-col items-start gap-2 sm:flex-row sm:items-center">
				<StyledInput
					v-model="newName"
					v-tooltip="advancedTooltip"
					wrapper-class="grow max-w-[400px]"
					:maxlength="32"
					:disabled="!canUseAdvancedSettings"
					:placeholder="formatMessage(messages.namePlaceholder)"
				/>
				<StyledInput
					v-if="isAdmin"
					v-model="newPort"
					type="number"
					wrapper-class="w-[9rem]"
					:disabled="!canUseAdvancedSettings"
					:placeholder="formatMessage(messages.portPlaceholder)"
				/>
				<Button
					v-tooltip="createTooltip"
					type="colored"
					color="brand"
					:disabled="!newName || creating || !canUseAdvancedSettings"
					@click="createAllocation"
				>
					<PlusIcon />
					{{ formatMessage(messages.create) }}
				</Button>
			</div>

			<LoadingIndicator v-if="loading" />
			<ErrorInformationCard
				v-else-if="error"
				:title="formatMessage(messages.loadFailed)"
				:description="error"
				:icon="IssuesIcon"
				:action="{ label: formatMessage(commonMessages.retryButton), onClick: () => void load() }"
			/>
			<Table v-else :columns="columns" :data="rows" row-key="port" table-min-width="28rem">
				<template #cell-name="{ index }">
					<TagItem v-if="rowAt(index).primary" class="!font-medium">
						{{ formatMessage(messages.primary) }}
					</TagItem>
					<span v-else class="font-semibold">{{ rowAt(index).name }}</span>
				</template>
				<template #cell-port="{ index }">
					<span class="font-medium">{{ rowAt(index).port }}</span>
				</template>
				<template #cell-actions="{ index }">
					<div class="flex items-center justify-end gap-2">
						<IconButton
							type="quiet"
							:label="formatMessage(commonMessages.copyIdButton)"
							@click="copy(`${host}:${rowAt(index).port}`)"
						>
							<CopyIcon />
						</IconButton>
						<template v-if="!rowAt(index).primary">
							<IconButton
								v-tooltip="promoteTooltip"
								type="quiet"
								:label="formatMessage(messages.makePrimary)"
								:disabled="!canUseAdvancedSettings || tunnelHolds"
								@click="makePrimary(rowAt(index).port)"
							>
								<StarIcon />
							</IconButton>
							<IconButton
								v-tooltip="advancedTooltip"
								type="quiet"
								:label="formatMessage(messages.rename)"
								:disabled="!canUseAdvancedSettings"
								@click="startRename(rowAt(index).port, rowAt(index).name)"
							>
								<EditIcon />
							</IconButton>
							<IconButton
								v-tooltip="advancedTooltip"
								type="quiet"
								:label="formatMessage(commonMessages.deleteLabel)"
								:disabled="!canUseAdvancedSettings"
								class="!text-red [&>svg]:!text-red"
								@click="startDelete(rowAt(index).port)"
							>
								<TrashIcon />
							</IconButton>
						</template>
					</div>
				</template>
			</Table>
			<span>{{ formatMessage(messages.allocationsHelp) }}</span>
		</div>

		<Teleport to="body">
			<div class="relative z-[100]">
				<NewModal ref="renameModal" :header="formatMessage(messages.rename)" width="550px">
					<form class="flex w-full flex-col gap-2" @submit.prevent="renameAllocation">
						<label for="allocation-name" class="font-semibold text-contrast">
							{{ formatMessage(messages.nameLabel) }}
						</label>
						<StyledInput
							id="allocation-name"
							v-model="renameValue"
							wrapper-class="w-full"
							:maxlength="32"
						/>
						<div class="mb-1 mt-4 flex justify-end gap-2.5">
							<Button @click="renameModal?.hide()">
								{{ formatMessage(commonMessages.cancelButton) }}
							</Button>
							<Button
								type="colored"
								color="brand"
								:disabled="!renameValue || saving"
								native-type="submit"
							>
								<SaveIcon />
								{{ formatMessage(commonMessages.saveButton) }}
							</Button>
						</div>
					</form>
				</NewModal>

				<ConfirmModal
					ref="deleteModal"
					:title="formatMessage(messages.deleteTitle)"
					:description="formatMessage(messages.deleteBody, { port: portToDelete ?? 0 })"
					:proceed-label="formatMessage(commonMessages.deleteLabel)"
					@proceed="deleteAllocation"
				/>
			</div>
		</Teleport>
	</div>
</template>

<script setup lang="ts">
import {
	CopyIcon,
	EditIcon,
	IssuesIcon,
	PlusIcon,
	SaveIcon,
	StarIcon,
	TrashIcon,
} from '@modrinth/assets'
import {
	Admonition,
	Button,
	commonMessages,
	ConfirmModal,
	CopyCode,
	defineMessages,
	ErrorInformationCard,
	IconButton,
	injectModrinthServerContext,
	injectNotificationManager,
	LoadingIndicator,
	NewModal,
	StyledInput,
	Table,
	type TableColumn,
	TagItem,
	useServerPermissions,
	useVIntl,
} from '@modrinth/ui'
import { computed, onMounted, ref, watch } from 'vue'

import { type Allocation, api } from '@/api'
import { shareAddress } from '@/api/playit'
import { actionsColumnWidth, ICON_BUTTON_REM } from '@/components/table-widths'
import { useServerTunnel } from '@/composables/playit'
import { useSession } from '@/composables/session'

import { publishedPort, tunnelHoldsPrimaryPort } from './playit-ports'

const { formatMessage } = useVIntl()
const { isAdmin } = useSession()
const { addNotification } = injectNotificationManager()
const { server, serverId } = injectModrinthServerContext()
const { canUseAdvancedSettings, permissionDeniedMessage } = useServerPermissions()
const { tunnel, available: playitAvailable } = useServerTunnel(serverId)

const messages = defineMessages({
	addressLabel: { id: 'craftpanel.settings.network.address', defaultMessage: 'Connection address' },
	addressHelp: {
		id: 'craftpanel.settings.network.address-help',
		defaultMessage: 'This is what players enter in their Minecraft client.',
	},
	addressHelpTunnelled: {
		id: 'craftpanel.settings.network.address-help-tunnelled',
		defaultMessage:
			'This is what players on this network enter. From outside, they use the public address below.',
	},
	publicTitle: {
		id: 'craftpanel.settings.network.public-title',
		defaultMessage: 'Published through playit.gg',
	},
	publishedAt: {
		id: 'craftpanel.settings.network.published-at',
		defaultMessage:
			'Port {port}, the primary one, is open to the internet as {address}. Everyone who has that address reaches this server.',
	},
	publishing: {
		id: 'craftpanel.settings.network.publishing',
		defaultMessage:
			'Port {port}, the primary one, is being published. playit.gg has not named the address yet.',
	},
	onlyThatOnePort: {
		id: 'craftpanel.settings.network.only-that-one-port',
		defaultMessage:
			'Only that one port. A server gets one tunnel, so every other allocation below stays reachable from this network alone.',
	},
	swapNeedsGiveBack: {
		id: 'craftpanel.settings.network.swap-needs-give-back',
		defaultMessage:
			'While that address stands, the primary port cannot be moved: where the tunnel points is kept at playit.gg and cannot be changed from here. The owner or a panel administrator gives the address back on the overview page; until then the panel refuses the swap.',
	},
	allocations: { id: 'craftpanel.settings.network.allocations', defaultMessage: 'Allocations' },
	allocationsHelp: {
		id: 'craftpanel.settings.network.allocations-help',
		defaultMessage:
			'Extra ports for things like map viewers or voice chat. A deleted port returns to the pool.',
	},
	name: { id: 'craftpanel.settings.network.column-name', defaultMessage: 'Name' },
	port: { id: 'craftpanel.settings.network.column-port', defaultMessage: 'Port' },
	actions: { id: 'craftpanel.settings.network.column-actions', defaultMessage: 'Actions' },
	primary: { id: 'craftpanel.settings.network.primary', defaultMessage: 'Primary' },
	makePrimary: { id: 'craftpanel.settings.network.make-primary', defaultMessage: 'Make primary' },
	swapBlocked: {
		id: 'craftpanel.settings.network.swap-blocked',
		defaultMessage: 'The public address holds the primary port',
	},
	rename: { id: 'craftpanel.settings.network.rename', defaultMessage: 'Rename allocation' },
	nameLabel: { id: 'craftpanel.settings.network.name-label', defaultMessage: 'Name' },
	namePlaceholder: {
		id: 'craftpanel.settings.network.name-placeholder',
		defaultMessage: 'e.g. Voice chat',
	},
	portPlaceholder: { id: 'craftpanel.settings.network.port-placeholder', defaultMessage: 'Port' },
	create: { id: 'craftpanel.settings.network.create', defaultMessage: 'Create allocation' },
	createHint: {
		id: 'craftpanel.settings.network.create-hint',
		defaultMessage: 'Enter a name to create an allocation.',
	},
	deleteTitle: { id: 'craftpanel.settings.network.delete-title', defaultMessage: 'Delete allocation' },
	deleteBody: {
		id: 'craftpanel.settings.network.delete-body',
		defaultMessage: 'Port {port} goes back into the pool and can be handed out again.',
	},
	loadFailed: {
		id: 'craftpanel.settings.network.load-failed',
		defaultMessage: 'Failed to load the allocations',
	},
	actionFailed: {
		id: 'craftpanel.settings.network.action-failed',
		defaultMessage: 'The change was refused',
	},
	created: { id: 'craftpanel.settings.network.created', defaultMessage: 'Allocation created' },
	renamed: { id: 'craftpanel.settings.network.renamed', defaultMessage: 'Allocation renamed' },
	deleted: { id: 'craftpanel.settings.network.deleted', defaultMessage: 'Allocation deleted' },
	promoted: { id: 'craftpanel.settings.network.promoted', defaultMessage: 'Primary port changed' },
	copied: { id: 'craftpanel.settings.network.copied', defaultMessage: 'Copied to clipboard' },
	restartHeader: {
		id: 'craftpanel.settings.network.restart-header',
		defaultMessage: 'Restart required',
	},
	restartBody: {
		id: 'craftpanel.settings.network.restart-body',
		defaultMessage: 'The new primary port takes effect the next time the server starts.',
	},
})

interface AllocationRow extends Record<string, unknown> {
	name: string
	port: number
	primary: boolean
}

const allocations = ref<Allocation[]>([])
const loading = ref(true)
const error = ref<string | null>(null)
const saving = ref(false)
const creating = ref(false)
const restartRequired = ref(false)
const newName = ref('')
const newPort = ref('')
const renameValue = ref('')
const portToRename = ref<number | null>(null)
const portToDelete = ref<number | null>(null)
const renameModal = ref<InstanceType<typeof NewModal>>()
const deleteModal = ref<InstanceType<typeof ConfirmModal>>()

const host = computed(() => server.value?.net?.ip ?? '')
const promotedPort = ref<number | null>(null)
const primaryPort = computed(() => promotedPort.value ?? server.value?.net?.port ?? 0)

watch(
	() => server.value?.net?.port,
	(port) => {
		if (port === promotedPort.value) promotedPort.value = null
	},
)
const address = computed(() =>
	host.value ? `${host.value}:${primaryPort.value}` : String(primaryPort.value),
)

const tunnelHolds = computed(() => tunnelHoldsPrimaryPort(tunnel.value, playitAvailable.value))
const published = computed(() => publishedPort(tunnel.value, primaryPort.value))
const publicAddress = computed(() => shareAddress(tunnel.value)?.address ?? null)

const advancedTooltip = computed(() =>
	canUseAdvancedSettings.value ? undefined : permissionDeniedMessage.value,
)
const promoteTooltip = computed(() => {
	if (!canUseAdvancedSettings.value) return permissionDeniedMessage.value
	if (tunnelHolds.value) return formatMessage(messages.swapBlocked)
	return formatMessage(messages.makePrimary)
})
const createTooltip = computed(() => {
	if (!canUseAdvancedSettings.value) return permissionDeniedMessage.value
	if (!newName.value) return formatMessage(messages.createHint)
	return undefined
})

const columns = computed<TableColumn[]>(() => [
	{ key: 'name', label: formatMessage(messages.name), width: '40%' },
	{ key: 'port', label: formatMessage(messages.port) },
	{
		key: 'actions',
		label: formatMessage(messages.actions),
		width: actionsColumnWidth(Array.from({ length: 4 }, () => ICON_BUTTON_REM)),
		align: 'right',
	},
])

function rowAt(index: number): AllocationRow {
	return rows.value[index]
}

const rows = computed<AllocationRow[]>(() => [
	{ name: formatMessage(messages.primary), port: primaryPort.value, primary: true },
	...allocations.value.map((allocation) => ({ ...allocation, primary: false })),
])

async function load(): Promise<void> {
	loading.value = true
	error.value = null
	try {
		allocations.value = await api.settings.allocations(serverId)
	} catch (cause) {
		error.value = cause instanceof Error ? cause.message : String(cause)
	} finally {
		loading.value = false
	}
}

onMounted(() => void load())

function report(cause: unknown): void {
	addNotification({
		type: 'error',
		title: formatMessage(messages.actionFailed),
		text: cause instanceof Error ? cause.message : undefined,
	})
}

async function copy(text: string): Promise<void> {
	await navigator.clipboard.writeText(text)
	addNotification({ type: 'success', title: formatMessage(messages.copied), text })
}

async function createAllocation(): Promise<void> {
	if (!canUseAdvancedSettings.value || !newName.value) return
	creating.value = true
	try {
		const port = Number.parseInt(newPort.value, 10)
		await api.settings.createAllocation(serverId, {
			name: newName.value,
			...(Number.isFinite(port) ? { port } : {}),
		})
		newName.value = ''
		newPort.value = ''
		await load()
		addNotification({ type: 'success', title: formatMessage(messages.created) })
	} catch (cause) {
		report(cause)
	} finally {
		creating.value = false
	}
}

function startRename(port: number, name: string): void {
	portToRename.value = port
	renameValue.value = name
	renameModal.value?.show()
}

async function renameAllocation(): Promise<void> {
	const port = portToRename.value
	if (port === null || !renameValue.value) return
	saving.value = true
	try {
		await api.settings.renameAllocation(serverId, port, { name: renameValue.value })
		renameModal.value?.hide()
		await load()
		addNotification({ type: 'success', title: formatMessage(messages.renamed) })
	} catch (cause) {
		report(cause)
	} finally {
		saving.value = false
	}
}

function startDelete(port: number): void {
	portToDelete.value = port
	deleteModal.value?.show()
}

async function deleteAllocation(): Promise<void> {
	const port = portToDelete.value
	if (port === null) return
	try {
		await api.settings.deleteAllocation(serverId, port)
		await load()
		addNotification({ type: 'success', title: formatMessage(messages.deleted) })
	} catch (cause) {
		report(cause)
	}
}

async function makePrimary(port: number): Promise<void> {
	if (!canUseAdvancedSettings.value || tunnelHolds.value) return
	try {
		const result = await api.settings.setPrimaryAllocation(serverId, port)
		allocations.value = result.allocations
		promotedPort.value = result.primary_port
		restartRequired.value = result.restart_required
		addNotification({ type: 'success', title: formatMessage(messages.promoted) })
	} catch (cause) {
		report(cause)
	}
}
</script>
