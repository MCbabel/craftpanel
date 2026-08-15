<template>
	<div class="flex flex-col gap-6">
		<div class="flex flex-col gap-1">
			<h1 class="m-0 text-2xl font-extrabold text-contrast">
				{{ formatMessage(messages.title) }}
			</h1>
			<p class="m-0 text-secondary">{{ formatMessage(messages.subtitle) }}</p>
		</div>

		<LoadingIndicator v-if="settings === null && loading" />

		<Admonition
			v-else-if="settings === null"
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
						:type="formatMessage(STATE_LABELS[settings.state])"
						:color="STATE_COLORS[settings.state]"
					/>
					<span v-if="settings.key_set_at" class="text-sm text-secondary">
						{{ formatMessage(messages.keySetAt, { when: relativeTime(settings.key_set_at) }) }}
					</span>
					<span v-else-if="settings.sink_path" class="text-sm text-secondary">
						{{ formatMessage(messages.sinkPath, { path: settings.sink_path }) }}
					</span>
					<span v-else class="text-sm text-secondary">
						{{ formatMessage(messages.noKeyYet) }}
					</span>
				</div>

				<dl class="m-0 grid grid-cols-2 gap-x-6 gap-y-2 sm:grid-cols-4">
					<div class="flex flex-col">
						<dt class="text-xs text-secondary">{{ formatMessage(messages.sentToday) }}</dt>
						<dd class="m-0 text-lg font-semibold text-contrast">
							{{
								settings.daily_limit === 0
									? formatNumber(settings.sent_today)
									: formatMessage(messages.ofLimit, {
											used: formatNumber(settings.sent_today),
											limit: formatNumber(settings.daily_limit),
										})
							}}
						</dd>
					</div>
					<div class="flex flex-col">
						<dt class="text-xs text-secondary">{{ formatMessage(messages.waiting) }}</dt>
						<dd class="m-0 text-lg font-semibold text-contrast">
							{{ formatNumber(settings.queued) }}
						</dd>
					</div>
					<div class="flex flex-col">
						<dt class="text-xs text-secondary">{{ formatMessage(messages.failedCount) }}</dt>
						<dd
							class="m-0 text-lg font-semibold"
							:class="settings.failed > 0 ? 'text-red' : 'text-contrast'"
						>
							{{ formatNumber(settings.failed) }}
						</dd>
					</div>
					<div class="flex flex-col">
						<dt class="text-xs text-secondary">{{ formatMessage(messages.lastTest) }}</dt>
						<dd class="m-0 text-sm text-contrast">
							{{
								settings.last_test_at
									? relativeTime(settings.last_test_at)
									: formatMessage(messages.never)
							}}
						</dd>
					</div>
				</dl>

				<Admonition
					v-if="settings.last_error"
					type="critical"
					:header="formatMessage(messages.lastErrorHeader)"
					:body="settings.last_error"
				/>
				<p class="m-0 text-xs text-secondary">{{ formatMessage(messages.acceptedNotArrived) }}</p>
			</Card>

			<Card v-if="settings.state === 'not_configured'" class="!mb-0 flex flex-col gap-3">
				<SettingsLabel
					:title="formatMessage(messages.howToTitle)"
					:description="formatMessage(messages.howToDescription)"
				/>
				<ol class="m-0 flex flex-col gap-2 pl-5 text-sm text-secondary">
					<li>
						{{ formatMessage(messages.step1) }}
						<a class="text-link hover:underline" href="https://resend.com/signup" target="_blank"
							>resend.com/signup</a
						>
					</li>
					<li>
						{{ formatMessage(messages.step2) }}
						<a class="text-link hover:underline" href="https://resend.com/api-keys" target="_blank"
							>resend.com/api-keys</a
						>
					</li>
					<li>{{ formatMessage(messages.step3) }}</li>
					<li>
						{{ formatMessage(messages.step4) }}
						<a class="text-link hover:underline" href="https://resend.com/domains" target="_blank"
							>resend.com/domains</a
						>
					</li>
				</ol>
				<Admonition type="info" :body="formatMessage(messages.freeTier)" />
			</Card>

			<Card class="!mb-0 flex flex-col gap-4">
				<SettingsLabel
					:title="formatMessage(messages.keyTitle)"
					:description="formatMessage(messages.keyDescription)"
				/>
				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="mail-key">
						{{ formatMessage(messages.keyLabel) }}
					</label>
					<StyledInput
						id="mail-key"
						v-model="keyInput"
						type="password"
						autocomplete="off"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						placeholder="re_…"
						wrapper-class="w-full sm:w-96"
					/>
					<span class="text-xs text-secondary">{{ formatMessage(messages.keyHint) }}</span>
				</div>
				<div class="flex flex-wrap gap-2.5">
					<Button
						type="colored"
						color="brand"
						:disabled="busy || keyInput.trim() === ''"
						@click="saveKey()"
					>
						<KeyIcon aria-hidden="true" />
						{{ formatMessage(messages.saveKey) }}
					</Button>
					<Button
						v-if="settings.key_set_at"
						:disabled="busy"
						@click="removeKeyModal?.show()"
					>
						<TrashIcon aria-hidden="true" />
						{{ formatMessage(messages.removeKey) }}
					</Button>
				</div>
			</Card>

			<Card class="!mb-0 flex flex-col gap-6">
				<SettingsLabel
					:title="formatMessage(messages.senderTitle)"
					:description="formatMessage(messages.senderDescription)"
				/>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="mail-from-address">
						{{ formatMessage(messages.fromAddress) }}
					</label>
					<StyledInput
						id="mail-from-address"
						v-model="draft.from_address"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:error="!addressLooksRight(draft.from_address)"
						wrapper-class="w-full sm:w-96"
					/>
					<span v-if="!addressLooksRight(draft.from_address)" class="text-sm font-medium text-red">
						{{ formatMessage(messages.addressInvalid) }}
					</span>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="mail-from-name">
						{{ formatMessage(messages.fromName) }}
					</label>
					<StyledInput
						id="mail-from-name"
						v-model="draft.from_name"
						wrapper-class="w-full sm:w-96"
					/>
					<span class="text-xs text-secondary">
						{{ formatMessage(messages.senderPreviewLabel) }}
						<span class="font-mono text-contrast">{{ senderLine }}</span>
					</span>
				</div>

				<Admonition
					v-if="!sendingToStrangersWorks(settings)"
					type="warning"
					:header="formatMessage(messages.testSenderHeader)"
					:body="formatMessage(messages.testSenderBody)"
				/>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="mail-reply-to">
						{{ formatMessage(messages.replyTo) }}
					</label>
					<StyledInput
						id="mail-reply-to"
						v-model="draft.reply_to"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:error="draft.reply_to.trim() !== '' && !addressLooksRight(draft.reply_to)"
						:placeholder="formatMessage(messages.replyToPlaceholder)"
						wrapper-class="w-full sm:w-96"
					/>
					<span class="text-xs text-secondary">{{ formatMessage(messages.replyToHint) }}</span>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="mail-link-base">
						{{ formatMessage(messages.linkBase) }}
					</label>
					<StyledInput
						id="mail-link-base"
						v-model="draft.link_base"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:error="linkProblem === 'no-scheme'"
						placeholder="https://panel.example"
						wrapper-class="w-full sm:w-96"
					/>
					<span v-if="example" class="break-all text-xs text-secondary">
						{{ formatMessage(messages.exampleLinkLabel) }}
						<span class="font-mono text-contrast">{{ example }}</span>
					</span>
					<Admonition
						v-if="linkProblem === 'missing'"
						type="warning"
						:body="formatMessage(messages.linkMissing)"
					/>
					<Admonition
						v-else-if="linkProblem === 'no-scheme'"
						type="critical"
						:body="formatMessage(messages.linkNoScheme)"
					/>
					<Admonition
						v-else-if="linkProblem === 'insecure'"
						type="warning"
						:body="formatMessage(messages.linkInsecure)"
					/>
				</div>

				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="mail-daily-limit">
						{{ formatMessage(messages.dailyLimit) }}
					</label>
					<StyledInput
						id="mail-daily-limit"
						:model-value="draft.daily_limit"
						type="number"
						:min="0"
						wrapper-class="w-32"
						@update:model-value="setDailyLimit"
					/>
					<span class="text-xs text-secondary">{{ formatMessage(messages.dailyLimitHint) }}</span>
				</div>

				<div class="flex flex-wrap items-center gap-2.5">
					<Button
						type="colored"
						color="brand"
						:disabled="busy || !formValid || !changed"
						@click="saveSettings()"
					>
						<SaveIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.saveChangesButton) }}
					</Button>
					<Button :disabled="busy || !changed" @click="draft = draftOf(settings)">
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
				</div>
			</Card>

			<Card class="!mb-0 flex flex-col gap-4">
				<SettingsLabel
					:title="formatMessage(messages.testTitle)"
					:description="formatMessage(messages.testDescription)"
				/>
				<div class="flex flex-col gap-1.5">
					<label class="text-sm font-semibold text-contrast" for="mail-test-to">
						{{ formatMessage(messages.testTo) }}
					</label>
					<StyledInput
						id="mail-test-to"
						v-model="testTo"
						autocapitalize="none"
						autocorrect="off"
						:spellcheck="false"
						:placeholder="formatMessage(messages.testToPlaceholder)"
						wrapper-class="w-full sm:w-96"
					/>
				</div>
				<div class="flex flex-wrap gap-2.5">
					<Button
						:disabled="busy || settings.state === 'not_configured'"
						@click="sendTest()"
					>
						<SendIcon aria-hidden="true" />
						{{ formatMessage(messages.sendTest) }}
					</Button>
				</div>
				<Admonition
					v-if="testResult"
					type="success"
					:header="formatMessage(messages.testSentHeader, { to: testResult.to })"
					:body="formatMessage(messages.testSentBody, { id: testResult.id })"
				/>
				<Admonition
					v-else-if="testFailure"
					type="critical"
					:header="formatMessage(messages.testFailedHeader)"
					:body="testFailure"
				/>
			</Card>

			<Card class="!mb-0 flex flex-col gap-3">
				<SettingsLabel
					:title="formatMessage(messages.previewTitle)"
					:description="formatMessage(messages.previewDescription)"
				/>
				<div class="flex flex-wrap gap-2">
					<ButtonLink
						v-for="kind in MAIL_KINDS"
						:key="kind"
						:href="previewUrl(kind)"
						target="_blank"
					>
						<EyeIcon aria-hidden="true" />
						{{ formatMessage(KIND_LABELS[kind]) }}
					</ButtonLink>
				</div>
				<p class="m-0 text-xs text-secondary">{{ formatMessage(messages.previewCli) }}</p>
			</Card>

			<Card class="!mb-0 flex flex-col gap-3">
				<div class="flex flex-wrap items-center justify-between gap-2">
					<SettingsLabel
						:title="formatMessage(messages.outboxTitle)"
						:description="formatMessage(messages.outboxDescription)"
					/>
					<Button :disabled="busy" @click="refresh()">
						<UpdatedIcon aria-hidden="true" />
						{{ formatMessage(commonMessages.refreshButton) }}
					</Button>
				</div>

				<Table
					:columns="columns"
					:data="rows"
					row-key="id"
					:table-min-width="wide ? '60rem' : undefined"
					:row-below-visible="!wide"
				>
					<template #empty-state>
						<EmptyState
							type="empty"
							:heading="formatMessage(messages.outboxEmpty)"
							:description="formatMessage(messages.outboxEmptyHint)"
						/>
					</template>

					<template #cell-mail="{ index }">
						<div class="flex min-w-0 flex-col">
							<span class="truncate font-medium text-contrast">
								{{ formatMessage(KIND_LABELS[at(index).kind]) }}
							</span>
							<span class="truncate text-xs text-secondary">{{ at(index).to_address }}</span>
						</div>
					</template>

					<template #cell-state="{ index }">
						<div class="flex min-w-0 flex-col gap-1">
							<Badge
								:type="formatMessage(DELIVERY_LABELS[at(index).state])"
								:color="DELIVERY_COLORS[at(index).state]"
							/>
							<span v-if="at(index).attempts > 0" class="text-xs text-secondary">
								{{ formatMessage(messages.attempts, { count: at(index).attempts }) }}
							</span>
						</div>
					</template>

					<template #cell-when="{ index }">
						<span class="text-sm text-contrast">{{ relativeTime(at(index).created_at) }}</span>
					</template>

					<template #cell-detail="{ index }">
						<span v-if="at(index).last_error" class="text-xs text-red">
							{{ at(index).last_error }}
						</span>
						<span v-else-if="at(index).provider_id" class="truncate font-mono text-xs text-secondary">
							{{ at(index).provider_id }}
						</span>
					</template>

					<template #cell-actions="{ index }">
						<div class="flex items-center justify-end gap-2">
							<ButtonLink
								v-if="at(index).has_content"
								:href="contentUrl(at(index).id)"
								target="_blank"
							>
								<EyeIcon aria-hidden="true" />
								<span class="sr-only">{{ formatMessage(messages.look) }}</span>
							</ButtonLink>
							<IconButton
								v-if="at(index).state === 'failed' && at(index).has_content"
								:label="formatMessage(messages.tryAgain)"
								:disabled="busy"
								@click="tryAgain(at(index).id)"
							>
								<RefreshCwIcon aria-hidden="true" />
							</IconButton>
						</div>
					</template>

					<template #row-below="{ index }">
						<dl class="m-0 grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2 px-4 pb-4 text-sm">
							<dt class="text-secondary">{{ formatMessage(messages.columnState) }}</dt>
							<dd class="m-0">
								<Badge
									:type="formatMessage(DELIVERY_LABELS[at(index).state])"
									:color="DELIVERY_COLORS[at(index).state]"
								/>
							</dd>
							<dt class="text-secondary">{{ formatMessage(messages.columnWhen) }}</dt>
							<dd class="m-0 text-contrast">{{ relativeTime(at(index).created_at) }}</dd>
							<template v-if="at(index).last_error">
								<dt class="text-secondary">{{ formatMessage(messages.columnDetail) }}</dt>
								<dd class="m-0 text-red">{{ at(index).last_error }}</dd>
							</template>
						</dl>
					</template>
				</Table>
			</Card>
		</template>

		<NewModal ref="removeKeyModal" :header="formatMessage(messages.removeKey)" width="34rem">
			<div class="flex flex-col gap-4">
				<p class="m-0">{{ formatMessage(messages.removeKeyBody) }}</p>
				<Admonition v-if="actionFailure" type="critical" :body="actionFailure" />
				<div class="mb-1 mt-2 flex flex-wrap justify-end gap-2.5">
					<Button :disabled="busy" @click="removeKeyModal?.hide()">
						{{ formatMessage(commonMessages.cancelButton) }}
					</Button>
					<Button type="colored" color="red" :disabled="busy" @click="removeKey()">
						<TrashIcon aria-hidden="true" />
						{{ formatMessage(messages.removeKey) }}
					</Button>
				</div>
			</div>
		</NewModal>
	</div>
</template>

<script setup lang="ts">
import {
	EyeIcon,
	KeyIcon,
	RefreshCwIcon,
	SaveIcon,
	SendIcon,
	TrashIcon,
	UpdatedIcon,
} from '@modrinth/assets'
import {
	Admonition,
	Badge,
	Button,
	ButtonLink,
	Card,
	commonMessages,
	defineMessages,
	EmptyState,
	IconButton,
	LoadingIndicator,
	NewModal,
	SettingsLabel,
	StyledInput,
	Table,
	type TableColumn,
	useFormatNumber,
	useRelativeTime,
	useVIntl,
} from '@modrinth/ui'
import { computed, ref } from 'vue'

import { isApiRequestError, type Ulid } from '@/api'
import {
	contentUrl,
	mail,
	MAIL_KINDS,
	type MailOutboxEntry,
	type MailSettings,
	previewUrl,
	type SendTestMailResponse,
} from '@/api/mail'
import {
	DELIVERY_COLORS,
	DELIVERY_LABELS,
	KIND_LABELS,
	STATE_COLORS,
	STATE_LABELS,
} from '@/components/mail-words'
import { actionsColumnWidth, ICON_BUTTON_REM, ICON_LABEL_BUTTON_REM } from '@/components/table-widths'
import { useWideScreen } from '@/composables/breakpoint'
import {
	addressLooksRight,
	type MailDraft,
	draftChanged,
	draftOf,
	draftRequest,
	exampleLink,
	linkBaseProblem,
	senderPreview,
	sendingToStrangersWorks,
} from '@/pages/admin/mail'

const { formatMessage } = useVIntl()
const formatNumber = useFormatNumber()
const relativeTime = useRelativeTime()
const wide = useWideScreen()

const messages = defineMessages({
	title: { id: 'admin.mail.title', defaultMessage: 'Mail' },
	subtitle: {
		id: 'admin.mail.subtitle',
		defaultMessage:
			'The panel sends mail through Resend: a sign-up confirmation, a password reset, a note when an account is let in. Nothing here has to be set up; without a key the panel sends nothing.',
	},
	loadFailed: { id: 'admin.mail.load-failed', defaultMessage: 'Could not load the mail settings' },
	unknownError: { id: 'admin.mail.unknown-error', defaultMessage: 'Something went wrong. Try again.' },
	keySetAt: { id: 'admin.mail.key-set-at', defaultMessage: 'Key stored {when}' },
	sinkPath: {
		id: 'admin.mail.sink-path',
		defaultMessage: 'Every mail is written to {path} and nothing goes to the network.',
	},
	noKeyYet: {
		id: 'admin.mail.no-key-yet',
		defaultMessage: 'No key here yet, so no mail leaves the panel.',
	},
	sentToday: { id: 'admin.mail.sent-today', defaultMessage: 'Sent in 24 h' },
	ofLimit: { id: 'admin.mail.of-limit', defaultMessage: '{used} of {limit}' },
	waiting: { id: 'admin.mail.waiting', defaultMessage: 'Waiting' },
	failedCount: { id: 'admin.mail.failed-count', defaultMessage: 'Failed' },
	lastTest: { id: 'admin.mail.last-test', defaultMessage: 'Last test mail' },
	never: { id: 'admin.mail.never', defaultMessage: 'never' },
	lastErrorHeader: { id: 'admin.mail.last-error-header', defaultMessage: 'The last mail did not go' },
	acceptedNotArrived: {
		id: 'admin.mail.accepted-not-arrived',
		defaultMessage:
			'“Accepted” means Resend took the mail, not that it reached the inbox. The panel cannot see a bounce.',
	},
	howToTitle: { id: 'admin.mail.how-to-title', defaultMessage: 'Where to get a key' },
	howToDescription: {
		id: 'admin.mail.how-to-description',
		defaultMessage: 'Four steps, and the first day works without a domain of your own.',
	},
	step1: { id: 'admin.mail.step1', defaultMessage: 'Open an account at' },
	step2: { id: 'admin.mail.step2', defaultMessage: 'Create an API key at' },
	step3: {
		id: 'admin.mail.step3',
		defaultMessage:
			'Give it the “Sending access” permission — the panel needs nothing more. The key begins with re_. Paste it below and save.',
	},
	step4: {
		id: 'admin.mail.step4',
		defaultMessage:
			'To write to anybody but yourself, verify a domain (MX and TXT records for SPF and DKIM) at',
	},
	freeTier: {
		id: 'admin.mail.free-tier',
		defaultMessage:
			'Resend’s free tier: 100 mails a day, 3,000 a month, one domain. Until a domain is verified, the sender can only be onboarding@resend.dev and the only recipient is the address your Resend account was opened with — enough for the test button below, not enough for real sign-ups.',
	},
	keyTitle: { id: 'admin.mail.key-title', defaultMessage: 'The Resend key' },
	keyDescription: {
		id: 'admin.mail.key-description',
		defaultMessage:
			'Stored as a file with mode 0600 next to the database, never in the database itself. A copy of panel.db is then not a way into your Resend account.',
	},
	keyLabel: { id: 'admin.mail.key-label', defaultMessage: 'API key' },
	keyHint: {
		id: 'admin.mail.key-hint',
		defaultMessage:
			'The field stays empty after saving: the panel never hands a key back out. Type a new one to replace it.',
	},
	saveKey: { id: 'admin.mail.save-key', defaultMessage: 'Save key' },
	removeKey: { id: 'admin.mail.remove-key', defaultMessage: 'Remove key' },
	removeKeyBody: {
		id: 'admin.mail.remove-key-body',
		defaultMessage:
			'The key file is deleted. No mail goes out afterwards, and sign-ups are closed until a new key is here. Mail that is still waiting fails with a note saying why. Put a new key in and “try again” sends it.',
	},
	senderTitle: { id: 'admin.mail.sender-title', defaultMessage: 'Sender and panel address' },
	senderDescription: {
		id: 'admin.mail.sender-description',
		defaultMessage: 'What your users see in their inbox, and where the links in a mail point.',
	},
	fromAddress: { id: 'admin.mail.from-address', defaultMessage: 'Sender address' },
	fromName: { id: 'admin.mail.from-name', defaultMessage: 'Sender name' },
	addressInvalid: {
		id: 'admin.mail.address-invalid',
		defaultMessage: 'That is not an address. The form is name@domain.tld.',
	},
	senderPreviewLabel: { id: 'admin.mail.sender-preview', defaultMessage: 'In the inbox:' },
	testSenderHeader: {
		id: 'admin.mail.test-sender-header',
		defaultMessage: 'This sender only reaches you',
	},
	testSenderBody: {
		id: 'admin.mail.test-sender-body',
		defaultMessage:
			'onboarding@resend.dev is Resend’s address for a first try: it delivers only to the address your Resend account was opened with. For real sign-ups, verify a domain at resend.com/domains and put an address of that domain in here.',
	},
	replyTo: { id: 'admin.mail.reply-to', defaultMessage: 'Reply address' },
	replyToPlaceholder: { id: 'admin.mail.reply-to-placeholder', defaultMessage: 'optional' },
	replyToHint: {
		id: 'admin.mail.reply-to-hint',
		defaultMessage:
			'Where an answer goes. Left empty, an answer goes to the sender address — and if nobody reads that, say so in the mail rather than here.',
	},
	linkBase: { id: 'admin.mail.link-base', defaultMessage: 'Panel address' },
	exampleLinkLabel: { id: 'admin.mail.example-link', defaultMessage: 'A link will look like:' },
	linkMissing: {
		id: 'admin.mail.link-missing',
		defaultMessage:
			'Without this address no mail with a link goes out — that is the confirmation mail, the password reset and the note to administrators. The four mails without a link still work.',
	},
	linkNoScheme: {
		id: 'admin.mail.link-no-scheme',
		defaultMessage: 'The address needs a scheme: https://panel.example (or http:// on a home network).',
	},
	linkInsecure: {
		id: 'admin.mail.link-insecure',
		defaultMessage:
			'Over http the token in the link travels in the clear. On a home network that is often the only choice; on the open internet use https.',
	},
	dailyLimit: { id: 'admin.mail.daily-limit', defaultMessage: 'Mails a day at most' },
	dailyLimitHint: {
		id: 'admin.mail.daily-limit-hint',
		defaultMessage:
			'The panel’s own brake, counted over the last 24 hours. 100 matches Resend’s free tier; 0 switches it off and leaves only Resend’s own limit.',
	},
	testTitle: { id: 'admin.mail.test-title', defaultMessage: 'Send a test mail' },
	testDescription: {
		id: 'admin.mail.test-description',
		defaultMessage:
			'Sent straight away, not queued: you are standing here, so you get the answer. It is a real mail with the real design, and it counts against the day’s allowance.',
	},
	testTo: { id: 'admin.mail.test-to', defaultMessage: 'To' },
	testToPlaceholder: {
		id: 'admin.mail.test-to-placeholder',
		defaultMessage: 'the address on your account',
	},
	sendTest: { id: 'admin.mail.send-test', defaultMessage: 'Send test mail' },
	testSentHeader: { id: 'admin.mail.test-sent-header', defaultMessage: 'Resend took the mail for {to}' },
	testSentBody: {
		id: 'admin.mail.test-sent-body',
		defaultMessage:
			'Resend’s id for it is {id}. If it does not arrive within a minute, look in the spam folder. That is what an unverified domain does.',
	},
	testFailedHeader: { id: 'admin.mail.test-failed-header', defaultMessage: 'The test mail did not go' },
	previewTitle: { id: 'admin.mail.preview-title', defaultMessage: 'Look at the mails' },
	previewDescription: {
		id: 'admin.mail.preview-description',
		defaultMessage:
			'Every mail with example values, in a new tab. Works without a key, without the network and without a single mail being sent.',
	},
	previewCli: {
		id: 'admin.mail.preview-cli',
		defaultMessage:
			'On the machine itself: craftpanel mail preview --out /tmp/craftpanel-mail writes all sixteen files, HTML and text.',
	},
	outboxTitle: { id: 'admin.mail.outbox-title', defaultMessage: 'The last mails' },
	outboxDescription: {
		id: 'admin.mail.outbox-description',
		defaultMessage:
			'Kept for 30 days. The body of a delivered mail is cleared: it carried a link that is a secret while the mail is in flight.',
	},
	outboxEmpty: { id: 'admin.mail.outbox-empty', defaultMessage: 'No mail yet' },
	outboxEmptyHint: {
		id: 'admin.mail.outbox-empty-hint',
		defaultMessage: 'Send the test mail above, and it will be the first line here.',
	},
	columnMail: { id: 'admin.mail.column.mail', defaultMessage: 'Mail' },
	columnState: { id: 'admin.mail.column.state', defaultMessage: 'State' },
	columnWhen: { id: 'admin.mail.column.when', defaultMessage: 'Queued' },
	columnDetail: { id: 'admin.mail.column.detail', defaultMessage: 'Detail' },
	columnActions: { id: 'admin.mail.column.actions', defaultMessage: 'Actions' },
	attempts: {
		id: 'admin.mail.attempts',
		defaultMessage: '{count, plural, one {# attempt} other {# attempts}}',
	},
	look: { id: 'admin.mail.look', defaultMessage: 'Look at the mail' },
	tryAgain: { id: 'admin.mail.try-again', defaultMessage: 'Try again' },
})

type MailColumn = 'mail' | 'state' | 'when' | 'detail' | 'actions'

const settings = ref<MailSettings | null>(null)
const outbox = ref<MailOutboxEntry[]>([])
const draft = ref<MailDraft>({
	from_address: '',
	from_name: '',
	reply_to: '',
	link_base: '',
	daily_limit: 100,
})
const keyInput = ref('')
const testTo = ref('')
const testResult = ref<SendTestMailResponse | null>(null)
const testFailure = ref<string | null>(null)
const loading = ref(true)
const busy = ref(false)
const loadFailure = ref<string | null>(null)
const actionFailure = ref<string | null>(null)
const removeKeyModal = ref<InstanceType<typeof NewModal> | null>(null)

const rows = computed(() => outbox.value.map((line) => ({ id: line.id, line })))
const changed = computed(() =>
	settings.value === null ? false : draftChanged(draft.value, settings.value),
)
const linkProblem = computed(() => linkBaseProblem(draft.value.link_base))
const example = computed(() => exampleLink(draft.value.link_base))
const senderLine = computed(() => senderPreview(draft.value.from_name, draft.value.from_address))
const formValid = computed(
	() =>
		addressLooksRight(draft.value.from_address) &&
		(draft.value.reply_to.trim() === '' || addressLooksRight(draft.value.reply_to)) &&
		linkProblem.value !== 'no-scheme' &&
		draft.value.daily_limit >= 0,
)

const columns = computed<TableColumn<MailColumn>[]>(() =>
	wide.value
		? [
				{ key: 'mail', label: formatMessage(messages.columnMail), width: '18rem' },
				{ key: 'state', label: formatMessage(messages.columnState), width: '10rem' },
				{ key: 'when', label: formatMessage(messages.columnWhen), width: '10rem' },
				{ key: 'detail', label: formatMessage(messages.columnDetail) },
				{
					key: 'actions',
					label: formatMessage(messages.columnActions),
					align: 'right',
					width: actionsColumnWidth([ICON_LABEL_BUTTON_REM, ICON_BUTTON_REM]),
				},
			]
		: [
				{ key: 'mail', label: formatMessage(messages.columnMail) },
				{
					key: 'actions',
					align: 'right',
					width: actionsColumnWidth([ICON_LABEL_BUTTON_REM, ICON_BUTTON_REM]),
				},
			],
)

function at(index: number): MailOutboxEntry {
	return rows.value[index]!.line
}

function reason(error: unknown): string {
	return isApiRequestError(error) ? error.message : formatMessage(messages.unknownError)
}

function setDailyLimit(value: string | number | undefined): void {
	const parsed = Number(value ?? 0)
	draft.value.daily_limit = Number.isFinite(parsed) ? Math.max(0, Math.trunc(parsed)) : 0
}

function adopt(fresh: MailSettings): void {
	const untouched = settings.value === null || !changed.value
	settings.value = fresh
	if (untouched) draft.value = draftOf(fresh)
}

async function load(): Promise<void> {
	loading.value = true
	loadFailure.value = null
	try {
		const [fresh, list] = await Promise.all([mail.settings(), mail.outbox({ limit: 50 })])
		adopt(fresh)
		outbox.value = list.mails
	} catch (error) {
		loadFailure.value = reason(error)
	} finally {
		loading.value = false
	}
}

async function refresh(): Promise<void> {
	try {
		const [fresh, list] = await Promise.all([mail.settings(), mail.outbox({ limit: 50 })])
		adopt(fresh)
		outbox.value = list.mails
	} catch (error) {
		actionFailure.value = reason(error)
	}
}

async function saveSettings(): Promise<void> {
	if (busy.value || settings.value === null) return
	busy.value = true
	actionFailure.value = null
	try {
		settings.value = await mail.save(draftRequest(draft.value))
		draft.value = draftOf(settings.value)
	} catch (error) {
		actionFailure.value = reason(error)
		loadFailure.value = null
	} finally {
		busy.value = false
	}
}

async function saveKey(): Promise<void> {
	if (busy.value || keyInput.value.trim() === '') return
	busy.value = true
	actionFailure.value = null
	try {
		settings.value = await mail.save(draftRequest(draft.value, keyInput.value.trim()))
		draft.value = draftOf(settings.value)
		keyInput.value = ''
	} catch (error) {
		actionFailure.value = reason(error)
	} finally {
		busy.value = false
	}
}

async function removeKey(): Promise<void> {
	if (busy.value) return
	busy.value = true
	actionFailure.value = null
	try {
		await mail.dropKey()
		removeKeyModal.value?.hide()
		await refresh()
	} catch (error) {
		actionFailure.value = reason(error)
	} finally {
		busy.value = false
	}
}

async function sendTest(): Promise<void> {
	if (busy.value) return
	busy.value = true
	testResult.value = null
	testFailure.value = null
	try {
		const typed = testTo.value.trim()
		testResult.value = await mail.test(typed === '' ? {} : { to: typed })
	} catch (error) {
		testFailure.value = reason(error)
	} finally {
		busy.value = false
		await refresh()
	}
}

async function tryAgain(id: Ulid): Promise<void> {
	if (busy.value) return
	busy.value = true
	actionFailure.value = null
	try {
		await mail.retry(id)
		await refresh()
	} catch (error) {
		actionFailure.value = reason(error)
	} finally {
		busy.value = false
	}
}

void load()
</script>
