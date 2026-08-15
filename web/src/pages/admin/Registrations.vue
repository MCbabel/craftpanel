<template>
	<div class="flex flex-col gap-6">
		<div class="flex flex-col gap-1">
			<h1 class="m-0 text-2xl font-extrabold text-contrast">
				{{ formatMessage(messages.title) }}
			</h1>
			<p class="m-0 text-secondary">{{ formatMessage(messages.subtitle) }}</p>
		</div>

		<Admonition
			v-if="signUpClosed"
			type="info"
			:header="formatMessage(messages.closedHeader)"
			:body="formatMessage(messages.closedBody)"
		>
			<template #actions>
				<Button @click="router.push({ name: 'admin-settings' })">
					<SettingsIcon aria-hidden="true" />
					{{ formatMessage(messages.openSettings) }}
				</Button>
			</template>
		</Admonition>

		<Admonition
			v-if="actionFailure"
			type="critical"
			:body="actionFailure"
			dismissible
			@dismiss="actionFailure = null"
		/>

		<LoadingIndicator v-if="rows === null && loading" />

		<Admonition
			v-else-if="rows === null"
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
			<p v-if="waiting > 0" class="m-0 text-sm text-secondary">
				{{ formatMessage(messages.waitingCount, { count: waiting }) }}
			</p>

			<Table
				:columns="columns"
				:data="tableRows"
				row-key="id"
				:table-min-width="wide ? '56rem' : undefined"
				:row-below-visible="!wide"
			>
				<template #empty-state>
					<EmptyState
						type="empty"
						:heading="formatMessage(messages.nobody)"
						:description="formatMessage(messages.nobodyHint)"
					/>
				</template>

				<template #cell-applicant="{ index }">
					<div class="flex min-w-0 flex-col">
						<span class="truncate font-medium text-contrast">{{ at(index).username }}</span>
						<span class="truncate text-xs text-secondary">{{ at(index).email }}</span>
					</div>
				</template>

				<template #cell-state="{ index }">
					<Badge :type="stateLabel(at(index))" :color="STATE_COLORS[at(index).state]" />
				</template>

				<template #cell-when="{ index }">
					<div class="flex min-w-0 flex-col">
						<span class="text-sm text-contrast">{{ relativeTime(at(index).created_at) }}</span>
						<span v-if="at(index).verified_at" class="truncate text-xs text-secondary">
							{{
								formatMessage(messages.confirmedAt, {
									when: relativeTime(at(index).verified_at!),
								})
							}}
						</span>
					</div>
				</template>

				<template #cell-from="{ index }">
					<div class="flex min-w-0 flex-col">
						<span class="truncate text-sm text-contrast">
							{{ at(index).signup_ip ?? formatMessage(messages.noAddress) }}
						</span>
						<span v-if="suspicious(rows ?? [], at(index))" class="text-xs text-orange">
							{{
								formatMessage(messages.sameAddress, {
									count: fromTheSameAddress(rows ?? [], at(index)),
								})
							}}
						</span>
					</div>
				</template>

				<template #cell-actions="{ index }">
					<div class="flex items-center justify-end gap-2">
						<Button
							type="colored"
							color="brand"
							:disabled="busy || !canApprove(at(index))"
							@click="approve(at(index))"
						>
							<CheckIcon aria-hidden="true" />
							<span class="sr-only md:not-sr-only">
								{{ formatMessage(messages.approve) }}
							</span>
						</Button>
						<Button :disabled="busy" @click="ask(at(index))">
							<XIcon aria-hidden="true" />
							<span class="sr-only md:not-sr-only">
								{{ formatMessage(messages.reject) }}
							</span>
						</Button>
					</div>
				</template>

				<template #row-below="{ index }">
					<dl class="m-0 grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2 px-4 pb-4 text-sm">
						<dt class="text-secondary">{{ formatMessage(messages.columnState) }}</dt>
						<dd class="m-0">
							<Badge :type="stateLabel(at(index))" :color="STATE_COLORS[at(index).state]" />
						</dd>
						<dt class="text-secondary">{{ formatMessage(messages.columnWhen) }}</dt>
						<dd class="m-0">{{ relativeTime(at(index).created_at) }}</dd>
						<dt class="text-secondary">{{ formatMessage(messages.columnFrom) }}</dt>
						<dd class="m-0 break-all">
							{{ at(index).signup_ip ?? formatMessage(messages.noAddress) }}
							<span v-if="suspicious(rows ?? [], at(index))" class="text-orange">
								{{
									formatMessage(messages.sameAddress, {
										count: fromTheSameAddress(rows ?? [], at(index)),
									})
								}}
							</span>
						</dd>
					</dl>
				</template>
			</Table>

			<p class="m-0 text-sm text-secondary">{{ formatMessage(messages.sweepHint) }}</p>
		</template>

		<NewModal ref="rejectModal" :header="formatMessage(messages.reject)" width="34rem">
			<div class="flex flex-col gap-4">
				<p class="m-0">
					{{ formatMessage(messages.rejectBody, { username: chosen?.username ?? '' }) }}
				</p>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="reason">
						{{ formatMessage(messages.reasonLabel) }}
					</label>
					<StyledInput id="reason" v-model="reason" :disabled="busy" wrapper-class="w-full" />
					<span class="text-xs text-secondary">{{ formatMessage(messages.reasonHint) }}</span>
				</div>

				<Admonition v-if="rejectFailure" type="critical" :body="rejectFailure" />

				<div class="mb-1 mt-2 flex flex-wrap justify-end gap-2.5">
					<Button :disabled="busy" @click="rejectModal?.hide()">
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button type="colored" color="red" :disabled="busy" @click="reject()">
						<XIcon aria-hidden="true" />
						{{ formatMessage(messages.reject) }}
					</Button>
				</div>
			</div>
		</NewModal>
	</div>
</template>

<script setup lang="ts">
import { CheckIcon, SettingsIcon, UpdatedIcon, XIcon } from '@modrinth/assets'
import {
	Admonition,
	Badge,
	Button,
	commonMessages,
	defineMessages,
	EmptyState,
	LoadingIndicator,
	NewModal,
	StyledInput,
	Table,
	type TableColumn,
	useRelativeTime,
	useVIntl,
} from '@modrinth/ui'
import { computed, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'

import { isApiRequestError, type Ulid } from '@/api'
import { type Registration, registrations } from '@/api/registration'
import { actionsColumnWidth, ICON_LABEL_BUTTON_REM } from '@/components/table-widths'
import { useWideScreen } from '@/composables/breakpoint'
import {
	canApprove,
	fromTheSameAddress,
	inWorkingOrder,
	queueProblem,
	suspicious,
	waitingForApproval,
	without,
} from '@/pages/admin/registrations'

const { formatMessage } = useVIntl()
const relativeTime = useRelativeTime()
const router = useRouter()
const wide = useWideScreen()

const messages = defineMessages({
	title: { id: 'admin.registrations.title', defaultMessage: 'Sign-ups' },
	subtitle: {
		id: 'admin.registrations.subtitle',
		defaultMessage:
			'People who signed themselves up. A confirmed one waits here until you let it in; nothing exists as an account before that.',
	},
	closedHeader: {
		id: 'admin.registrations.closed-header',
		defaultMessage: 'Sign-ups are switched off',
	},
	closedBody: {
		id: 'admin.registrations.closed-body',
		defaultMessage:
			'Nobody can sign up at the moment, so nothing new will appear here. What is listed can still be let in or turned away.',
	},
	openSettings: { id: 'admin.registrations.open-settings', defaultMessage: 'Panel settings' },
	loadFailed: {
		id: 'admin.registrations.load-failed',
		defaultMessage: 'Could not load the sign-ups',
	},
	unknownError: {
		id: 'admin.registrations.unknown-error',
		defaultMessage: 'Something went wrong. Try again.',
	},
	nobody: { id: 'admin.registrations.nobody', defaultMessage: 'Nobody is waiting' },
	nobodyHint: {
		id: 'admin.registrations.nobody-hint',
		defaultMessage: 'New sign-ups appear here as soon as somebody fills the form in.',
	},
	waitingCount: {
		id: 'admin.registrations.waiting-count',
		defaultMessage: '{count, plural, one {# sign-up is} other {# sign-ups are}} waiting for you.',
	},
	columnApplicant: { id: 'admin.registrations.column.applicant', defaultMessage: 'Applicant' },
	columnState: { id: 'admin.registrations.column.state', defaultMessage: 'State' },
	columnWhen: { id: 'admin.registrations.column.when', defaultMessage: 'Signed up' },
	columnFrom: { id: 'admin.registrations.column.from', defaultMessage: 'From' },
	stateUnverified: {
		id: 'admin.registrations.state.unverified',
		defaultMessage: 'Address not confirmed',
	},
	stateWaiting: { id: 'admin.registrations.state.waiting', defaultMessage: 'Waiting for you' },
	confirmedAt: { id: 'admin.registrations.confirmed-at', defaultMessage: 'confirmed {when}' },
	noAddress: { id: 'admin.registrations.no-address', defaultMessage: 'not recorded' },
	sameAddress: {
		id: 'admin.registrations.same-address',
		defaultMessage: '{count} from this address',
	},
	approve: { id: 'admin.registrations.approve', defaultMessage: 'Let in' },
	reject: { id: 'admin.registrations.reject', defaultMessage: 'Turn away' },
	rejectBody: {
		id: 'admin.registrations.reject-body',
		defaultMessage:
			'{username} gets a short, neutral mail without a reason, and the address is blocked for thirty days.',
	},
	reasonLabel: {
		id: 'admin.registrations.reason-label',
		defaultMessage: 'Reason (optional, for you)',
	},
	reasonHint: {
		id: 'admin.registrations.reason-hint',
		defaultMessage: 'Stays in the panel. The mail never carries it.',
	},
	sweepHint: {
		id: 'admin.registrations.sweep-hint',
		defaultMessage:
			'Unconfirmed sign-ups disappear after seven days, confirmed ones after thirty; the name and the address are free again then.',
	},
	failed: {
		id: 'admin.registrations.failed',
		defaultMessage: 'That did not work. Please try again.',
	},
})

const problems = defineMessages({
	registration_not_found: {
		id: 'admin.registrations.error.not-found',
		defaultMessage: 'That sign-up is gone — somebody else decided, or it was swept.',
	},
	invalid_state: {
		id: 'admin.registrations.error.invalid-state',
		defaultMessage: 'This one has not confirmed its address yet, so it cannot be let in.',
	},
	username_taken: {
		id: 'admin.registrations.error.username-taken',
		defaultMessage: 'That name has been taken in the meantime. The applicant has to sign up again.',
	},
	email_taken: {
		id: 'admin.registrations.error.email-taken',
		defaultMessage: 'That address is already on an account.',
	},
	failed: {
		id: 'admin.registrations.error.failed',
		defaultMessage: 'That did not work. Please try again.',
	},
})

const STATE_COLORS: Record<Registration['state'], string> = {
	email_unverified: 'gray',
	awaiting_approval: 'orange',
}

const rows = ref<Registration[] | null>(null)
const loading = ref(false)
const busy = ref(false)
const loadFailure = ref<string | null>(null)
const actionFailure = ref<string | null>(null)
const rejectFailure = ref<string | null>(null)
const signUpClosed = ref(false)
const chosen = ref<Registration | null>(null)
const reason = ref('')
const rejectModal = ref<InstanceType<typeof NewModal> | null>(null)

const ordered = computed(() => inWorkingOrder(rows.value ?? []))
const waiting = computed(() => waitingForApproval(rows.value ?? []))

type QueueColumn = 'applicant' | 'state' | 'when' | 'from' | 'actions'
type QueueRow = { id: Ulid; line: Registration }

const tableRows = computed<QueueRow[]>(() =>
	ordered.value.map((line) => ({ id: line.id, line })),
)

const columns = computed<TableColumn<QueueColumn>[]>(() =>
	wide.value
		? [
				{ key: 'applicant', label: formatMessage(messages.columnApplicant), width: '18rem' },
				{ key: 'state', label: formatMessage(messages.columnState), width: '12rem' },
				{ key: 'when', label: formatMessage(messages.columnWhen), width: '11rem' },
				{ key: 'from', label: formatMessage(messages.columnFrom), width: '12rem' },
				{
					key: 'actions',
					label: '',
					align: 'right',
					width: actionsColumnWidth([ICON_LABEL_BUTTON_REM, ICON_LABEL_BUTTON_REM]),
				},
			]
		: [
				{ key: 'applicant', label: formatMessage(messages.columnApplicant) },
				{
					key: 'actions',
					label: '',
					align: 'right',
					width: actionsColumnWidth([ICON_LABEL_BUTTON_REM, ICON_LABEL_BUTTON_REM]),
				},
			],
)

function at(index: number): Registration {
	return ordered.value[index]!
}

function stateLabel(row: Registration): string {
	return formatMessage(
		row.state === 'awaiting_approval' ? messages.stateWaiting : messages.stateUnverified,
	)
}

onMounted(load)

async function load() {
	loading.value = true
	loadFailure.value = null
	try {
		rows.value = (await registrations.queue()).registrations
	} catch (error) {
		loadFailure.value = isApiRequestError(error) ? error.message : null
	} finally {
		loading.value = false
	}

	try {
		signUpClosed.value = !(await registrations.options()).registration_enabled
	} catch {
	}
}

async function approve(row: Registration) {
	if (busy.value) return
	busy.value = true
	actionFailure.value = null
	try {
		const account = await registrations.approve(row.id)
		rows.value = without(rows.value ?? [], row.id)
		if (account.system_user.state === 'error') {
			actionFailure.value = account.system_user.error_message ?? formatMessage(messages.failed)
		}
	} catch (error) {
		const code = isApiRequestError(error) ? error.code : ''
		actionFailure.value = formatMessage(problems[queueProblem(code)])
		if (queueProblem(code) === 'registration_not_found') rows.value = without(rows.value ?? [], row.id)
	} finally {
		busy.value = false
	}
}

function ask(row: Registration) {
	chosen.value = row
	reason.value = ''
	rejectFailure.value = null
	rejectModal.value?.show()
}

async function reject() {
	const row = chosen.value
	if (busy.value || row === null) return
	busy.value = true
	rejectFailure.value = null
	try {
		await registrations.reject(row.id, reason.value)
		rows.value = without(rows.value ?? [], row.id)
		rejectModal.value?.hide()
	} catch (error) {
		const code = isApiRequestError(error) ? error.code : ''
		rejectFailure.value = formatMessage(problems[queueProblem(code)])
	} finally {
		busy.value = false
	}
}
</script>
