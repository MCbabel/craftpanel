<template>
	<div class="flex flex-col gap-6">
		<div class="flex flex-col gap-1">
			<h1 class="m-0 text-2xl font-extrabold text-contrast">
				{{ formatMessage(messages.title) }}
			</h1>
			<p class="m-0 text-secondary">{{ formatMessage(messages.subtitle) }}</p>
		</div>

		<LoadingIndicator v-if="overview === null && loading" />

		<Admonition
			v-else-if="overview === null"
			type="critical"
			:header="formatMessage(messages.loadFailed)"
			:body="loadFailure ?? formatMessage(messages.unknownError)"
		>
			<template #actions>
				<Button @click="load()">
					<UpdatedIcon aria-hidden="true" />
					{{ formatMessage(commonMessages.retryButton) }}
				</Button>
			</template>
		</Admonition>

		<template v-else>
			<Card class="!mb-0 flex flex-col gap-4">
				<div class="flex flex-wrap items-center gap-3">
					<Badge
						:type="formatMessage(overview.configured ? messages.stateReady : messages.stateBlank)"
						:color="overview.configured ? 'green' : 'orange'"
					/>
					<span class="text-sm text-secondary">
						{{
							overview.configured
								? formatMessage(messages.stateReadyHint)
								: formatMessage(messages.stateBlankHint)
						}}
					</span>
				</div>

				<ol class="m-0 flex list-decimal flex-col gap-2 pl-5 text-secondary">
					<li>
						{{ formatMessage(messages.stepProject) }}
						<a
							class="text-link"
							href="https://console.cloud.google.com/projectcreate"
							target="_blank"
							rel="noopener noreferrer"
						>
							console.cloud.google.com
							<ExternalIcon aria-hidden="true" class="inline size-3" />
						</a>
					</li>
					<li>
						{{ formatMessage(messages.stepApi) }}
						<a
							class="text-link"
							href="https://console.cloud.google.com/apis/library/drive.googleapis.com"
							target="_blank"
							rel="noopener noreferrer"
						>
							Drive API
							<ExternalIcon aria-hidden="true" class="inline size-3" />
						</a>
					</li>
					<li>{{ formatMessage(messages.stepConsent) }}</li>
					<li class="font-semibold text-contrast">{{ formatMessage(messages.stepPublish) }}</li>
					<li>{{ formatMessage(messages.stepClient) }}</li>
				</ol>

				<Admonition
					type="warning"
					:header="formatMessage(messages.publishHeader)"
					:body="formatMessage(messages.publishBody)"
				/>
			</Card>

			<Card class="!mb-0 flex flex-col gap-4">
				<SettingsLabel
					:title="formatMessage(messages.credentialsTitle)"
					:description="formatMessage(messages.credentialsDescription)"
				/>

				<label class="flex flex-col gap-1">
					<span class="text-sm font-semibold text-contrast">
						{{ formatMessage(messages.clientId) }}
					</span>
					<input
						v-model="draft.client_id"
						class="w-full"
						type="text"
						autocomplete="off"
						spellcheck="false"
						:placeholder="formatMessage(messages.clientIdPlaceholder)"
					/>
				</label>

				<label class="flex flex-col gap-1">
					<span class="text-sm font-semibold text-contrast">
						{{ formatMessage(messages.clientSecret) }}
					</span>
					<input
						v-model="draft.client_secret"
						class="w-full"
						type="password"
						autocomplete="off"
						spellcheck="false"
						:placeholder="
							overview.configured
								? formatMessage(messages.clientSecretKeep)
								: formatMessage(messages.clientSecretPlaceholder)
						"
					/>
					<span class="text-xs text-secondary">
						{{ formatMessage(messages.clientSecretHint) }}
					</span>
				</label>

				<label class="flex flex-col gap-1">
					<span class="text-sm font-semibold text-contrast">
						{{ formatMessage(messages.folderName) }}
					</span>
					<input v-model="draft.folder_name" class="w-full" type="text" spellcheck="false" />
					<span class="text-xs text-secondary">{{ formatMessage(messages.folderHint) }}</span>
				</label>

				<fieldset class="m-0 flex flex-col gap-2 border-0 p-0">
					<legend class="mb-1 p-0 text-sm font-semibold text-contrast">
						{{ formatMessage(messages.policy) }}
					</legend>
					<label
						v-for="option in POLICIES"
						:key="option"
						class="flex cursor-pointer items-start gap-2"
					>
						<input
							v-model="draft.target_policy"
							class="mt-1"
							type="radio"
							name="drive-policy"
							:value="option"
						/>
						<span class="flex flex-col">
							<span class="font-semibold text-contrast">
								{{ formatMessage(POLICY_LABELS[option]) }}
							</span>
							<span class="text-sm text-secondary">
								{{ formatMessage(POLICY_HINTS[option]) }}
							</span>
						</span>
					</label>
				</fieldset>

				<Admonition v-if="saveFailure" type="critical" :body="saveFailure" />
				<Admonition v-if="saved" type="success" :body="formatMessage(messages.savedBody)" />

				<div class="flex flex-wrap gap-2">
					<Button type="colored" color="brand" :disabled="busy" @click="save">
						<SaveIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.saveChangesButton) }}
					</Button>
					<Button
						v-if="overview.configured"
						type="colored"
						color="red"
						:disabled="busy"
						@click="forgetModal?.show()"
					>
						<TrashIcon aria-hidden="true" />
						{{ formatMessage(messages.forget) }}
					</Button>
				</div>
			</Card>

			<Card class="!mb-0 flex flex-col gap-4">
				<SettingsLabel
					:title="formatMessage(messages.accountsTitle)"
					:description="formatMessage(messages.accountsDescription)"
				/>

				<Table
					:columns="columns"
					:data="rows"
					row-key="user_id"
					:table-min-width="wide ? '72rem' : undefined"
					:row-below-visible="!wide"
				>
					<template #empty-state>
						<EmptyState
							type="empty"
							:heading="formatMessage(messages.accountsEmpty)"
							:description="formatMessage(messages.accountsEmptyHint)"
						/>
					</template>

					<template #cell-user="{ index }">
						<div class="flex min-w-0 flex-col">
							<span class="truncate font-semibold text-contrast">{{ at(index).username }}</span>
							<span v-if="at(index).google_email" class="truncate text-xs text-secondary">
								{{ at(index).google_email }}
							</span>
						</div>
					</template>

					<template #cell-state="{ index }">
						<div class="flex min-w-0 flex-col gap-1">
							<Badge :type="stateLabel(index)" :color="stateColor(index)" />
							<span v-if="at(index).last_error" class="truncate text-xs text-secondary">
								{{ at(index).last_error }}
							</span>
						</div>
					</template>

					<template #cell-storage="{ index }">
						<span class="text-sm text-contrast">{{ storageLabel(index) }}</span>
					</template>

					<template #cell-day="{ index }">
						<span class="text-sm text-contrast">{{ dayLabel(index) }}</span>
					</template>

					<template #cell-backups="{ index }">
						<span class="text-sm text-contrast">{{ backupsLabel(index) }}</span>
					</template>

					<template #cell-checked="{ index }">
						<span class="text-sm text-secondary">{{ checkedLabel(index) }}</span>
					</template>

					<template #cell-actions="{ index }">
						<div class="flex items-center justify-end">
							<Button :disabled="busy" @click="askToCut(at(index))">
								<UnlinkIcon aria-hidden="true" />
								<span class="sr-only md:not-sr-only">{{ formatMessage(messages.cut) }}</span>
							</Button>
						</div>
					</template>

					<template #row-below="{ index }">
						<dl class="m-0 grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2 px-4 pb-4 text-sm">
							<dt class="text-secondary">{{ formatMessage(messages.columnState) }}</dt>
							<dd class="m-0 flex min-w-0 flex-col gap-1">
								<Badge :type="stateLabel(index)" :color="stateColor(index)" />
								<span v-if="at(index).last_error" class="text-xs text-secondary">
									{{ at(index).last_error }}
								</span>
							</dd>

							<dt class="text-secondary">{{ formatMessage(messages.columnStorage) }}</dt>
							<dd class="m-0 text-contrast">{{ storageLabel(index) }}</dd>

							<dt class="text-secondary">{{ formatMessage(messages.columnDay) }}</dt>
							<dd class="m-0 text-contrast">{{ dayLabel(index) }}</dd>

							<dt class="text-secondary">{{ formatMessage(messages.columnBackups) }}</dt>
							<dd class="m-0 text-contrast">{{ backupsLabel(index) }}</dd>

							<dt class="text-secondary">{{ formatMessage(messages.columnChecked) }}</dt>
							<dd class="m-0 text-contrast">{{ checkedLabel(index) }}</dd>
						</dl>
					</template>
				</Table>
			</Card>
		</template>

		<NewModal ref="forgetModal" :header="formatMessage(messages.forget)" width="34rem">
			<div class="flex flex-col gap-4">
				<p class="m-0">{{ formatMessage(messages.forgetBody) }}</p>
				<Admonition type="warning" :body="formatMessage(messages.forgetWarning)" />
				<Admonition v-if="saveFailure" type="critical" :body="saveFailure" />

				<div class="mb-1 mt-2 flex justify-end gap-2.5">
					<Button :disabled="busy" @click="forgetModal?.hide()">
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button type="colored" color="red" :disabled="busy" @click="forget">
						<TrashIcon aria-hidden="true" />
						{{ formatMessage(messages.forget) }}
					</Button>
				</div>
			</div>
		</NewModal>

		<NewModal ref="cutModal" :header="formatMessage(messages.cut)" width="34rem">
			<div class="flex flex-col gap-4">
				<p class="m-0">
					{{ formatMessage(messages.cutBody, { user: cutting?.username ?? '' }) }}
				</p>
				<Admonition type="warning" :body="formatMessage(messages.cutFiles)" />
				<Admonition v-if="cutFailure" type="critical" :body="cutFailure" />

				<div class="mb-1 mt-2 flex justify-end gap-2.5">
					<Button :disabled="busy" @click="cutModal?.hide()">
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button type="colored" color="red" :disabled="busy" @click="cut">
						<UnlinkIcon aria-hidden="true" />
						{{ formatMessage(messages.cut) }}
					</Button>
				</div>
			</div>
		</NewModal>
	</div>
</template>

<script setup lang="ts">
import { ExternalIcon, SaveIcon, TrashIcon, UnlinkIcon, UpdatedIcon } from '@modrinth/assets'
import {
	Admonition,
	Badge,
	Button,
	Card,
	commonMessages,
	defineMessages,
	EmptyState,
	LoadingIndicator,
	NewModal,
	SettingsLabel,
	type TableColumn,
	Table,
	useFormatBytes,
	useFormatNumber,
	useRelativeTime,
	useVIntl,
} from '@modrinth/ui'
import { computed, ref } from 'vue'

import { isApiRequestError, type Ulid } from '@/api'
import {
	type BackupTargetPolicy,
	drive,
	type DriveAdminOverview,
	type DriveOverview,
} from '@/api/drive'
import { actionsColumnWidth, ICON_LABEL_BUTTON_REM } from '@/components/table-widths'
import { useWideScreen } from '@/composables/breakpoint'
import { useFormatDecimalBytes } from '@/composables/format-bytes'

import { blankDraft, type DriveDraft, draftOf, POLICIES } from './drive'

const { formatMessage } = useVIntl()
const formatBytes = useFormatBytes()
const decimalBytes = useFormatDecimalBytes()
const formatNumber = useFormatNumber()
const relativeTime = useRelativeTime()
const wide = useWideScreen()

const messages = defineMessages({
	title: { id: 'admin.drive.title', defaultMessage: 'Google Drive' },
	subtitle: {
		id: 'admin.drive.subtitle',
		defaultMessage:
			'Set up one Google project here; every user then connects his own account, and the backups go into his storage instead of onto this machine.',
	},
	loadFailed: { id: 'admin.drive.load-failed', defaultMessage: 'Could not load the Drive settings' },
	unknownError: {
		id: 'admin.drive.unknown-error',
		defaultMessage: 'Something went wrong. Try again.',
	},
	stateReady: { id: 'admin.drive.state.ready', defaultMessage: 'Set up' },
	stateBlank: { id: 'admin.drive.state.blank', defaultMessage: 'Not set up' },
	stateReadyHint: {
		id: 'admin.drive.state.ready-hint',
		defaultMessage: 'Users can connect their Google account on their own account page.',
	},
	stateBlankHint: {
		id: 'admin.drive.state.blank-hint',
		defaultMessage: 'Backups stay on this machine, and nothing in the panel calls Google.',
	},
	stepProject: { id: 'admin.drive.step.project', defaultMessage: 'Create a project at' },
	stepApi: {
		id: 'admin.drive.step.api',
		defaultMessage: 'Switch on the Drive API — without it every call is refused:',
	},
	stepConsent: {
		id: 'admin.drive.step.consent',
		defaultMessage:
			'Fill in the OAuth consent screen: audience “External”, an app name, a support address, and the scope .../auth/drive.file. That scope is not a sensitive one, so no review is needed.',
	},
	stepPublish: {
		id: 'admin.drive.step.publish',
		defaultMessage: 'Press “Publish app” so the publishing status is “In production”.',
	},
	stepClient: {
		id: 'admin.drive.step.client',
		defaultMessage:
			'Create a client of type “TVs and Limited Input devices” and put its id and secret in below.',
	},
	publishHeader: {
		id: 'admin.drive.publish.header',
		defaultMessage: 'Publishing the app is not optional',
	},
	publishBody: {
		id: 'admin.drive.publish.body',
		defaultMessage:
			'While the consent screen is on “Testing”, Google hands out access that expires after seven days: every connection then breaks after a week, silently, and exactly when somebody needs a backup. The panel cannot ask Google for this setting; it can only tell you about it.',
	},
	credentialsTitle: { id: 'admin.drive.credentials.title', defaultMessage: 'Client credentials' },
	credentialsDescription: {
		id: 'admin.drive.credentials.description',
		defaultMessage:
			'The id is kept in the database; the secret is written to a file that only the panel can read, and it is never shown again.',
	},
	clientId: { id: 'admin.drive.client-id', defaultMessage: 'Client ID' },
	clientIdPlaceholder: {
		id: 'admin.drive.client-id.placeholder',
		defaultMessage: '1234567890-abcdef.apps.googleusercontent.com',
	},
	clientSecret: { id: 'admin.drive.client-secret', defaultMessage: 'Client secret' },
	clientSecretPlaceholder: {
		id: 'admin.drive.client-secret.placeholder',
		defaultMessage: 'GOCSPX-…',
	},
	clientSecretKeep: {
		id: 'admin.drive.client-secret.keep',
		defaultMessage: 'A secret is stored — leave empty to keep it',
	},
	clientSecretHint: {
		id: 'admin.drive.client-secret.hint',
		defaultMessage: 'Leave empty to keep the stored secret. Saving with this field cleared removes it.',
	},
	folderName: { id: 'admin.drive.folder-name', defaultMessage: 'Folder name' },
	folderHint: {
		id: 'admin.drive.folder-hint',
		defaultMessage:
			'The folder the panel makes in each user’s Drive. Renaming it here does not rename the folders that already exist.',
	},
	policy: { id: 'admin.drive.policy', defaultMessage: 'Where backups may go' },
	policyUserChoice: { id: 'admin.drive.policy.user-choice', defaultMessage: 'Let each server choose' },
	policyUserChoiceHint: {
		id: 'admin.drive.policy.user-choice-hint',
		defaultMessage:
			'Every server backs up here by default; whoever has connected a Drive can switch his servers over.',
	},
	policyDriveOnly: { id: 'admin.drive.policy.drive-only', defaultMessage: 'Google Drive only' },
	policyDriveOnlyHint: {
		id: 'admin.drive.policy.drive-only-hint',
		defaultMessage:
			'Nothing is kept on this machine, and a user who has not connected a Drive cannot back up at all. That is deliberate: a silent fall back to this disk is how somebody ends up believing he is covered.',
	},
	policyLocalOnly: { id: 'admin.drive.policy.local-only', defaultMessage: 'This machine only' },
	policyLocalOnlyHint: {
		id: 'admin.drive.policy.local-only-hint',
		defaultMessage:
			'The way out if Google causes trouble. Backups already in a Drive stay readable and restorable; only new ones stay here.',
	},
	savedBody: { id: 'admin.drive.saved', defaultMessage: 'Saved.' },
	forget: { id: 'admin.drive.forget', defaultMessage: 'Remove credentials' },
	forgetBody: {
		id: 'admin.drive.forget.body',
		defaultMessage: 'The client id and the secret are deleted, and the panel is “not set up” again.',
	},
	forgetWarning: {
		id: 'admin.drive.forget.warning',
		defaultMessage:
			'The connections your users have made stay listed but stop working: no backup can go into a Drive and none can be fetched back out until credentials are entered again.',
	},
	accountsTitle: { id: 'admin.drive.accounts.title', defaultMessage: 'Connected accounts' },
	accountsDescription: {
		id: 'admin.drive.accounts.description',
		defaultMessage:
			'One line per user who has connected a Google account. You cannot connect one for anybody — only the person in front of the browser can do that.',
	},
	accountsEmpty: {
		id: 'admin.drive.accounts.empty',
		defaultMessage: 'Nobody has connected a Google account',
	},
	accountsEmptyHint: {
		id: 'admin.drive.accounts.empty-hint',
		defaultMessage:
			'Whoever wants his backups in his own storage connects his Google account on his account page. There is nothing to do here for him.',
	},
	columnUser: { id: 'admin.drive.column.user', defaultMessage: 'User' },
	columnState: { id: 'admin.drive.column.state', defaultMessage: 'State' },
	columnStorage: { id: 'admin.drive.column.storage', defaultMessage: 'Drive storage' },
	columnDay: { id: 'admin.drive.column.day', defaultMessage: 'Sent today' },
	dayValue: { id: 'admin.drive.day-value', defaultMessage: '{sent} of {limit}' },
	columnBackups: { id: 'admin.drive.column.backups', defaultMessage: 'Backups there' },
	columnChecked: { id: 'admin.drive.column.checked', defaultMessage: 'Last checked' },
	columnActions: { id: 'admin.drive.column.actions', defaultMessage: 'Actions' },
	stateConnected: { id: 'admin.drive.account.connected', defaultMessage: 'Connected' },
	stateRevoked: { id: 'admin.drive.account.revoked', defaultMessage: 'Access withdrawn' },
	stateError: { id: 'admin.drive.account.error', defaultMessage: 'Not working' },
	stateConnecting: { id: 'admin.drive.account.connecting', defaultMessage: 'Nothing connected' },
	storageValue: { id: 'admin.drive.storage-value', defaultMessage: '{used} of {limit}' },
	storageUnlimited: { id: 'admin.drive.storage-unlimited', defaultMessage: 'No limit' },
	backupsValue: { id: 'admin.drive.backups-value', defaultMessage: '{count} · {bytes}' },
	checkedNever: { id: 'admin.drive.checked-never', defaultMessage: 'never' },
	cut: { id: 'admin.drive.cut', defaultMessage: 'Disconnect' },
	cutBody: {
		id: 'admin.drive.cut.body',
		defaultMessage:
			'The panel gives its access to {user}’s Google account back and forgets it. He can connect again himself at any time.',
	},
	cutFiles: {
		id: 'admin.drive.cut.files',
		defaultMessage:
			'His backups stay in his Drive. There is no button here that deletes files in somebody else’s storage, and that is on purpose: he can see them and throw them away himself.',
	},
})

const STATE_LABELS = {
	connected: messages.stateConnected,
	revoked: messages.stateRevoked,
	error: messages.stateError,
} as const

const STATE_COLORS = { connected: 'green', revoked: 'red', error: 'orange' } as const

function stateLabel(index: number): string {
	const state = at(index).state
	return formatMessage(state === null ? messages.stateConnecting : STATE_LABELS[state])
}

function stateColor(index: number): string {
	const state = at(index).state
	return state === null ? 'blue' : STATE_COLORS[state]
}

const POLICY_LABELS: Record<BackupTargetPolicy, typeof messages.policyUserChoice> = {
	user_choice: messages.policyUserChoice,
	drive_only: messages.policyDriveOnly,
	local_only: messages.policyLocalOnly,
}

const POLICY_HINTS: Record<BackupTargetPolicy, typeof messages.policyUserChoiceHint> = {
	user_choice: messages.policyUserChoiceHint,
	drive_only: messages.policyDriveOnlyHint,
	local_only: messages.policyLocalOnlyHint,
}

type DriveColumn = 'user' | 'state' | 'storage' | 'day' | 'backups' | 'checked' | 'actions'
type DriveRow = { user_id: Ulid; line: DriveOverview }

const overview = ref<DriveAdminOverview | null>(null)
const draft = ref<DriveDraft>(blankDraft())
const cutting = ref<DriveOverview | null>(null)
const loading = ref(true)
const busy = ref(false)
const saved = ref(false)
const loadFailure = ref<string | null>(null)
const saveFailure = ref<string | null>(null)
const cutFailure = ref<string | null>(null)
const forgetModal = ref<InstanceType<typeof NewModal> | null>(null)
const cutModal = ref<InstanceType<typeof NewModal> | null>(null)

const rows = computed<DriveRow[]>(() =>
	(overview.value?.accounts ?? []).map((line) => ({ user_id: line.user_id, line })),
)

const columns = computed<TableColumn<DriveColumn>[]>(() =>
	wide.value
		? [
				{ key: 'user', label: formatMessage(messages.columnUser), width: '18rem' },
				{ key: 'state', label: formatMessage(messages.columnState), width: '12rem' },
				{ key: 'storage', label: formatMessage(messages.columnStorage), width: '12rem' },
				{ key: 'day', label: formatMessage(messages.columnDay), width: '12rem' },
				{ key: 'backups', label: formatMessage(messages.columnBackups), width: '10rem' },
				{ key: 'checked', label: formatMessage(messages.columnChecked), width: '10rem' },
				{
					key: 'actions',
					label: formatMessage(messages.columnActions),
					align: 'right',
					width: '9rem',
				},
			]
		: [
				{ key: 'user', label: formatMessage(messages.columnUser) },
				{ key: 'actions', align: 'right', width: actionsColumnWidth([ICON_LABEL_BUTTON_REM]) },
			],
)

function at(index: number): DriveOverview {
	return rows.value[index]!.line
}

function storageLabel(index: number): string {
	const account = at(index)
	if (account.storage_limit_bytes === null) return formatMessage(messages.storageUnlimited)
	return formatMessage(messages.storageValue, {
		used: formatBytes(account.storage_usage_bytes ?? 0),
		limit: formatBytes(account.storage_limit_bytes),
	})
}

function dayLabel(index: number): string {
	const account = at(index)
	return formatMessage(messages.dayValue, {
		sent: decimalBytes(account.uploaded_today_bytes),
		limit: decimalBytes(account.daily_upload_limit_bytes),
	})
}

function backupsLabel(index: number): string {
	return formatMessage(messages.backupsValue, {
		count: formatNumber(at(index).backups),
		bytes: formatBytes(at(index).backup_bytes),
	})
}

function checkedLabel(index: number): string {
	const when = at(index).checked_at
	return when ? relativeTime(when) : formatMessage(messages.checkedNever)
}

function reason(error: unknown): string {
	return isApiRequestError(error) ? error.message : formatMessage(messages.unknownError)
}

function adopt(next: DriveAdminOverview): void {
	overview.value = next
	draft.value = draftOf(next)
}

async function load(): Promise<void> {
	loading.value = true
	loadFailure.value = null
	try {
		adopt(await drive.overview())
	} catch (error) {
		loadFailure.value = reason(error)
	} finally {
		loading.value = false
	}
}

void load()

async function save(): Promise<void> {
	if (busy.value) return
	busy.value = true
	saveFailure.value = null
	saved.value = false
	try {
		adopt(
			await drive.save({
				client_id: draft.value.client_id.trim() === '' ? null : draft.value.client_id.trim(),
				client_secret: draft.value.client_secret === '' ? undefined : draft.value.client_secret,
				target_policy: draft.value.target_policy,
				folder_name: draft.value.folder_name.trim(),
			}),
		)
		saved.value = true
	} catch (error) {
		saveFailure.value = reason(error)
	} finally {
		busy.value = false
	}
}

async function forget(): Promise<void> {
	if (busy.value) return
	busy.value = true
	saveFailure.value = null
	try {
		await drive.forgetCredentials()
		forgetModal.value?.hide()
		await load()
	} catch (error) {
		saveFailure.value = reason(error)
	} finally {
		busy.value = false
	}
}

function askToCut(account: DriveOverview): void {
	cutting.value = account
	cutFailure.value = null
	cutModal.value?.show()
}

async function cut(): Promise<void> {
	const account = cutting.value
	if (busy.value || account === null) return
	busy.value = true
	cutFailure.value = null
	try {
		await drive.disconnectUser(account.user_id)
		cutModal.value?.hide()
		await load()
	} catch (error) {
		cutFailure.value = reason(error)
	} finally {
		busy.value = false
	}
}
</script>
