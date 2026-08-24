export type Ulid = string
export type Rfc3339 = string
export type FilePath = string

export interface ApiError {
	error: string
	message: string
}

export type PanelRole = 'admin' | 'user'

export type ServerRole = 'owner' | 'editor' | 'viewer'

export type PermissionBit =
	| 'BASE_READ'
	| 'POWER_ACTIONS'
	| 'EXEC_COMMANDS'
	| 'FILES_WRITE'
	| 'SETUP'
	| 'BACKUPS'
	| 'ADVANCED'
	| 'RESET_SERVER'
	| 'MANAGE_USERS'
	| 'SERVER_ADMIN'

export type PermissionMask = string

export interface UserRef {
	id: Ulid
	username: string
	avatar_url: string | null
}

export type OperationKind =
	| 'server_create'
	| 'server_delete'
	| 'install_loader'
	| 'repair_content'
	| 'reset_server'
	| 'install_modpack'
	| 'install_content'
	| 'update_content'
	| 'change_game_version'
	| 'install_java'
	| 'backup_create'
	| 'backup_restore'
	| 'unarchive'

export type OperationState = 'queued' | 'ongoing' | 'done' | 'failed' | 'cancelled'

export type OperationPhase =
	| 'analyzing'
	| 'installing_loader'
	| 'verifying'
	| 'running_installer'
	| 'installing_java'
	| 'installing_pack'
	| 'addons'
	| 'writing_config'

export type OperationErrorStep = 'modloader' | 'modpack' | 'download' | 'filesystem' | 'internal'

export interface OperationError {
	code: string
	message: string
	step: OperationErrorStep
}

export interface Operation {
	id: Ulid
	server_id: Ulid
	kind: OperationKind
	state: OperationState
	phase: OperationPhase | null
	progress: number
	message: string | null
	src: FilePath | null
	bytes_processed: number | null
	files_processed: number | null
	current_file: string | null
	error: OperationError | null
	cancellable: boolean
	target_id: Ulid | null
	started_by: Ulid | null
	created_at: Rfc3339
	started_at: Rfc3339 | null
	finished_at: Rfc3339 | null
	dismissed_at: Rfc3339 | null
}

export type BusyReasonCode =
	| 'installing'
	| 'syncing_content'
	| 'backup_creating'
	| 'backup_restoring'
	| 'deleting'

export interface OperationListResponse {
	revision: number
	operations: Operation[]
	busy_reasons: BusyReasonCode[]
}

export interface AllOperationsResponse {
	operations: Operation[]
	busy_reasons_by_server: Record<Ulid, BusyReasonCode[]>
}

export interface OperationAccepted {
	operation: Operation
}

export type ServerStatus = 'installing' | 'available' | 'broken' | 'deleting'

export type LoaderId =
	| 'vanilla'
	| 'paper'
	| 'folia'
	| 'purpur'
	| 'leaf'
	| 'fabric'
	| 'velocity'
	| 'neoforge'
	| 'quilt'
	| 'forge'

export interface ServerNet {
	ip: string | null
	port: number
	domain: string
}

export interface ServerUpstream {
	kind: 'modpack'
	project_id: string
	version_id: string
}

export interface Server {
	id: Ulid
	name: string
	owner_id: Ulid
	status: ServerStatus
	game: 'Minecraft'
	loader: LoaderId | null
	loader_version: string | null
	game_version: string | null
	net: ServerNet
	memory_mib: number
	upstream: ServerUpstream | null
	flows: { intro: boolean }
	backup_quota: number
	used_backup_quota: number
	update_channel: UpdateChannel
	current_user_permissions: PermissionMask
	created_at: Rfc3339
}

export interface ServerListResponse {
	servers: Server[]
	users: Record<Ulid, UserRef>
}

export interface KnownProperties {
	allow_cheats?: string | null
	allow_flight?: string | null
	difficulty?: string | null
	enforce_whitelist?: string | null
	force_gamemode?: string | null
	gamemode?: string | null
	generate_structures?: string | null
	generator_settings?: string | null
	hardcore?: string | null
	level_seed?: string | null
	level_type?: string | null
	max_players?: string | null
	max_tick_time?: string | null
	motd?: string | null
	pause_when_empty_seconds?: string | null
	player_idle_timeout?: string | null
	require_resource_pack?: string | null
	resource_pack?: string | null
	resource_pack_id?: string | null
	resource_pack_sha1?: string | null
	simulation_distance?: string | null
	spawn_protection?: string | null
	sync_chunk_writes?: string | null
	view_distance?: string | null
	white_list?: string | null
}

export interface PropertiesFields {
	known: KnownProperties
	custom?: Record<string, string>
}

export type CreateServerContent =
	| {
			kind: 'loader'
			loader: LoaderId
			game_version: string
			loader_version: string | null
	  }
	| { kind: 'modpack_project'; project_id: string; version_id: string }
	| { kind: 'modpack_upload'; file_name: string; file_size: number }

export interface CreateServerRequest {
	name: string
	owner_id: Ulid | null
	memory_mib: number
	port: number | null
	eula_accepted: boolean
	content: CreateServerContent
	properties: PropertiesFields
}

export type ServerWarning = 'memory_overcommitted' | 'properties_will_be_ignored'

export interface CreateServerResponse {
	server: Server
	operation: Operation
	warnings?: ServerWarning[]
}

export interface UpdateServerRequest {
	name?: string
	update_channel?: UpdateChannel
}

export type PowerAction = 'start' | 'stop' | 'restart' | 'kill'
export type PowerState = 'stopped' | 'starting' | 'running' | 'stopping' | 'crashed'
export type PowerTarget = 'start' | 'stop' | 'restart'

export interface PowerRequest {
	action: PowerAction
}

export interface PowerResponse {
	power_state: PowerState
	target: PowerTarget | null
}

export interface SendCommandRequest {
	command: string
}

export type CrashAnalysisSource = 'latest_log' | 'buffer'

export interface CrashAnalysisRequest {
	source?: CrashAnalysisSource
}

export interface CrashAnalysisEntry {
	level: number
	time: string | null
	prefix: string
	lines: Array<{ number: number; content: string }>
}

export interface CrashAnalysisResponse {
	id: string
	name: string | null
	type: string
	version: string | null
	title: string
	analysis: {
		problems: Array<{
			message: string
			counter: number
			entry: CrashAnalysisEntry
			solutions: Array<{ message: string }>
		}>
		information: Array<{
			message: string
			counter: number
			label: string
			value: string
			entry: CrashAnalysisEntry
		}>
	}
}

export type LogFileKind = 'log' | 'crash_report'

export interface LogFile {
	file: FilePath
	name: string
	kind: LogFileKind
	size_bytes: number
	modified_at: Rfc3339
	compressed: boolean
}

export interface LogFileListResponse {
	total: number
	truncated: boolean
	files: LogFile[]
}

export interface LogFileContentResponse {
	file: FilePath
	size_bytes: number
	content_bytes: number
	truncated: boolean
	content: string
}

export const CONSOLE_SERVER_BUFFER_LINES = 10_000
export const CONSOLE_SERVER_BUFFER_BYTES = 4 * 1024 * 1024
export const CONSOLE_CLIENT_BUFFER_LINES = 25_000
export const CONSOLE_CLIENT_BUFFER_BYTES = 8 * 1024 * 1024
export const CONSOLE_HISTORY_CHUNK_LINES = 500
export const CONSOLE_MAX_LINE_BYTES = 8192
export const CONSOLE_MAX_COMMAND_BYTES = 8192
export const LOG_GUNZIP_MAX_BYTES = 512 * 1024 * 1024
export const PANEL_LINE_TAG = 'Panel'

export interface ApiFileItem {
	name: string
	type: 'file' | 'directory' | 'symlink'
	path: FilePath
	modified: number
	created: number
	size?: number
	count?: number
	target?: string
}

export interface FilesMetaResponse {
	root_path: string
	max_upload_bytes: number
	max_text_bytes: number
	max_page_size: number
	default_page_size: number
	max_extract_uncompressed_bytes: number
	max_extract_entries: number
}

export interface ListDirectoryQuery {
	path?: FilePath
	after?: string
	page_size?: number
}

export interface ListDirectoryResponse {
	path: FilePath
	page_size: number
	total: number
	has_more: boolean
	next_after: string | null
	items: ApiFileItem[]
}

export interface CreateItemRequest {
	path: FilePath
	type: 'file' | 'directory'
}

export interface CreateItemResponse {
	item: ApiFileItem
}

export interface MoveItemRequest {
	source: FilePath
	destination: FilePath
	overwrite?: boolean
}

export interface MoveItemResponse {
	moved: boolean
}

export interface DeleteItemQuery {
	path: FilePath
	recursive?: boolean
}

export interface ReadContentQuery {
	path: FilePath
	max_bytes?: number
	download?: 0 | 1
}

export type WriteConflictMode = 'overwrite' | 'fail'

export interface WriteContentQuery {
	path: FilePath
	on_conflict?: WriteConflictMode
}

export interface ExtractRequest {
	path: FilePath
	target?: FilePath | null
	override: boolean
	dry: boolean
}

export interface ExtractDryRunResponse {
	modpack_name: string | null
	conflicting_files: FilePath[]
}

export type ContentProjectType = 'mod' | 'plugin' | 'datapack' | 'resourcepack' | 'shader'
export type ContentSourceKind = 'local' | 'modrinth_modpack' | 'server_project'
export type UpdateChannel = 'release' | 'beta' | 'alpha'

export interface ModrinthOwner {
	id: string
	name: string
	type: 'user' | 'organization'
	avatar_url: string | null
}

export interface ContentProject {
	id: string
	slug: string | null
	title: string
	icon_url: string | null
}

export interface ContentVersion {
	id: string
	version_number: string
	file_name: string
	date_published: Rfc3339 | null
}

export interface ApiContentItem {
	id: Ulid
	file_name: string
	file_path: FilePath
	size: number
	enabled: boolean
	locked: boolean
	project_type: ContentProjectType
	date_added: Rfc3339
	source_kind: ContentSourceKind
	environment: string | null
	pack_client_retained: boolean
	pack_client_depends: boolean
	installing: boolean
	external: boolean
	external_url: string | null
	has_update: boolean
	update_version_id: string | null
	project_id: string | null
	project: ContentProject | null
	version: ContentVersion | null
	owner: ModrinthOwner | null
}

export interface ContentModpack {
	source_kind: 'modrinth_modpack' | 'local'
	project_id: string | null
	slug: string | null
	title: string
	description: string | null
	icon_url: string | null
	filename: string | null
	downloads: number | null
	followers: number | null
	owner: ModrinthOwner | null
	categories: string[]
	version_id: string | null
	version_number: string | null
	date_published: Rfc3339 | null
	has_update: boolean
	update_version_id: string | null
}

export interface ContentListResponse {
	content_type: ContentProjectType
	loader: LoaderId
	loader_version: string | null
	game_version: string
	update_channel: UpdateChannel
	updates_checked_at: Rfc3339 | null
	permissions: { can_read: boolean; can_write: boolean }
	modpack: ContentModpack | null
	items: ApiContentItem[]
	truncated: boolean
}

export interface ModpackContentsResponse {
	items: ApiContentItem[]
}

export interface ContentIdsRequest {
	ids: Ulid[]
}

export interface ContentMutationResult {
	id: Ulid
	ok: boolean
	file_name: string | null
	file_path: FilePath | null
	enabled: boolean | null
	error: string | null
	message: string | null
}

export interface ContentMutationResponse {
	results: ContentMutationResult[]
}

export interface ContentUpdateTarget {
	id: Ulid
	version_id: string | null
}

export interface ContentUpdateRequest {
	items: ContentUpdateTarget[]
	all: boolean
}

export interface ContentUpdateResponse {
	operation: Operation
	total: number
}

export interface ContentInstallTarget {
	project_id: string
	version_id: string | null
}

export interface ContentInstallRequest {
	items: ContentInstallTarget[]
	resolve_dependencies: boolean
}

export type ContentSkipReason =
	| 'already_installed'
	| 'duplicate_project'
	| 'conflicting_dependency'
	| 'no_compatible_version'
	| 'missing_version'
	| 'quilt_fabric_api'

export interface ContentPlanEntry {
	project_id: string
	version_id: string
	file_name: string
	reason: 'requested' | 'dependency'
}

export interface ContentSkippedEntry {
	project_id: string
	version_id: string | null
	reason: ContentSkipReason
}

export interface ContentInstallResponse {
	operation: Operation
	planned: ContentPlanEntry[]
	skipped: ContentSkippedEntry[]
}

export interface ContentUploadResult {
	file_name: string
	ok: boolean
	id: Ulid | null
	error: string | null
	message: string | null
}

export interface ContentUploadResponse {
	results: ContentUploadResult[]
}

export interface ContentDependentEntry {
	id: Ulid
	depends_on: Ulid[]
}

export interface ContentDependentsResponse {
	dependents: ContentDependentEntry[]
}

export type ModpackSource =
	| { kind: 'modrinth'; project_id: string; version_id: string | null }
	| { kind: 'upload' }

export interface ModpackInstallRequest {
	source: ModpackSource
	keep_extra_content: boolean
}

export interface ModpackUpdateRequest {
	version_id: string | null
}

export interface ModpackUnlinkResponse {
	unlinked: boolean
	adopted_items: number
}

export type GameVersionChangeDiffType =
	| 'added'
	| 'removed'
	| 'updated'
	| 'modpack_unlinked'
	| 'game_version_updated'
	| 'loader_updated'
	| 'config_files_updated'

export interface GameVersionChangeVersion {
	id: string
	version_number: string
}

export interface GameVersionChangeEntry {
	type: GameVersionChangeDiffType
	id: Ulid | null
	file_name: string | null
	project_id: string | null
	project_title: string | null
	project_icon_url: string | null
	current_version: GameVersionChangeVersion | null
	new_version: GameVersionChangeVersion | null
}

export interface GameVersionPreviewResponse {
	new_game_version: string
	new_loader: LoaderId
	new_loader_version: string | null
	has_unknown_content: boolean
	changes: GameVersionChangeEntry[]
}

export interface GameVersionChangeRequest {
	game_version: string
	loader: LoaderId | null
	loader_version: string | null
	incompatible_content: 'update_then_disable' | 'disable' | 'keep'
}

export interface ServerProperties {
	known: KnownProperties
	custom: Record<string, string>
	restart_required: boolean
}

export interface ServerPropertiesPatch {
	known?: Record<string, string | null>
	custom?: Record<string, string | null>
}

export type JreVendor = 'temurin' | 'corretto' | 'graal'

export interface StartupOptions {
	java_version: number | null
	jre_vendor: JreVendor | null
	java_path: string | null
	memory_mib: number
	memory_max_mib: number
	extra_flags: string[]
	startup_command: string
	original_invocation: string
	managed_flags: string[]
	stripped_flags: string[]
	restart_required: boolean
}

export interface StartupOptionsPatch {
	java_version?: number | null
	jre_vendor?: JreVendor | null
	memory_mib?: number
	startup_command?: string | null
}

export interface JavaRuntime {
	major: number
	vendor: JreVendor
	version: string
	path: string | null
	source: 'system' | 'managed'
	installed: boolean
}

export interface JavaRuntimeList {
	runtimes: JavaRuntime[]
	default_major_for_game_version: number | null
}

export interface JavaInstallJob {
	stage: 'waiting' | 'asking' | 'downloading' | 'unpacking' | 'done'
	running: boolean
	done_bytes: number
	total_bytes: number
	share: number
	failure: string | null
	failure_code: string | null
}

export interface LaidJavaRuntime {
	vendor: JreVendor
	version: string
	path: string
	directory: string
	size_bytes: number
	laid_at: Rfc3339 | null
}

export interface SystemJavaRuntime {
	vendor: JreVendor
	version: string
	path: string
}

export interface JavaMajorEntry {
	major: number
	fetchable: boolean
	runtime: LaidJavaRuntime | null
	system: SystemJavaRuntime | null
	job: JavaInstallJob | null
	servers: number
	running: string[]
}

export interface JavaRuntimeOverview {
	auto_install: boolean
	architecture: string | null
	directory: string
	total_bytes: number
	majors: JavaMajorEntry[]
}

export interface Allocation {
	port: number
	name: string
}

export type AllocationList = Allocation[]

export interface CreateAllocationRequest {
	name: string
	port?: number
}

export interface RenameAllocationRequest {
	name: string
}

export interface SetPrimaryResponse {
	primary_port: number
	allocations: Allocation[]
	restart_required: boolean
}

export interface LoaderInfo {
	id: LoaderId
	name: string
	kind: 'vanilla' | 'server' | 'modloader' | 'proxy'
	install_kind: 'download' | 'installer'
	has_loader_versions: boolean
	supports_properties: boolean
	supports_content: boolean
	source:
		| 'mojang'
		| 'papermc'
		| 'purpurmc'
		| 'leafmc'
		| 'fabricmc'
		| 'neoforged'
		| 'quiltmc'
		| 'minecraftforge'
	wave: 1 | 2
}

export interface LoaderList {
	loaders: LoaderInfo[]
}

export interface GameVersionEntry {
	version: string
	version_type: 'release' | 'snapshot'
}

export interface GameVersionList {
	loader: LoaderId
	game_versions: GameVersionEntry[]
	cached_until: Rfc3339
}

export interface LoaderBuild {
	id: string
	label: string
	stable: boolean
	channel_tag: 'ALPHA' | 'BETA' | null
	released: Rfc3339 | null
}

export interface LoaderBuildList {
	loader: LoaderId
	game_version: string
	builds: LoaderBuild[]
	truncated: boolean
	cached_until: Rfc3339
}

export type ContentPolicy = 'keep' | 'wipe_mods'

export interface InstallRequest {
	loader: LoaderId
	game_version: string
	loader_version: string | null
	content_policy: ContentPolicy
}

export interface ResetRequest {
	loader: LoaderId
	game_version: string
	loader_version: string | null
	keep_backups: true
}

export interface InstallAccepted {
	operation: Operation
	warnings?: ServerWarning[]
}

export interface ResetToSetupResponse {
	server_id: Ulid
	flows: { intro: boolean }
}

export type BackupStatus = 'pending' | 'in_progress' | 'timed_out' | 'error' | 'done'
export type BackupOperationType = 'create' | 'restore'

export type BackupOperationState =
	| 'pending'
	| 'ongoing'
	| 'completed'
	| 'cancelled'
	| 'failed'
	| 'timed_out'

export interface BackupOperation {
	operation_type: BackupOperationType
	operation_id: Ulid
	state: BackupOperationState
	scheduled_for: Rfc3339
	started_at: Rfc3339 | null
	completed_at: Rfc3339 | null
	has_parent: boolean
	error: string | null
	should_prompt: boolean
	synthetic_legacy: false
	user_info: UserRef | null
}

export interface BackupActiveOperation {
	backup_id: Ulid
	operation_type: BackupOperationType
	operation_id: Ulid
	has_parent: boolean
	scheduled_for: Rfc3339
	started_at: Rfc3339 | null
	synthetic_legacy: false
	user_info: UserRef | null
}

export interface Backup {
	id: Ulid
	name: string
	created_at: Rfc3339
	status: BackupStatus
	locked: false
	automated: boolean
	size_bytes: number
	location: BackupLocation
	drive_state: DriveFileState | null
	drive_verified: boolean | null
	drive_content_changed: boolean | null
	drive_web_link: string | null
	history: BackupOperation[]
}

export type BackupLocation = 'local' | 'drive'

export type DriveFileState = 'present' | 'missing' | 'trashed' | 'unreachable'

export interface BackupListResponse {
	active_operations: BackupActiveOperation[]
	backups: Backup[]
}

export interface CreateBackupRequest {
	name: string
}

export interface RenameBackupRequest {
	name: string
}

export interface RestoreBackupRequest {
	name: string
}

export interface RestoreBackupResponse {
	restore_operation_id: Ulid
	safety_backup: { id: Ulid; create_operation_id: Ulid }
}

export interface RetryBackupResponse {
	operation_id: Ulid
	operation_type: BackupOperationType
}

export interface BulkDeleteBackupsRequest {
	backup_ids: Ulid[]
}

export interface BulkDeleteBackupsResponse {
	deleted: Ulid[]
	failed: Array<{ id: Ulid; error: string; message: string }>
}

export type BackupScheduleStatus =
	| 'completed'
	| 'failed'
	| 'timed_out'
	| 'skipped_unchanged'
	| 'skipped_limit'

export interface BackupSchedule {
	enabled: boolean
	interval_hours: number
	hour_utc: number
	keep_last: number
	next_run_at: Rfc3339 | null
	last_run_at: Rfc3339 | null
	last_status: BackupScheduleStatus | null
	last_error: string | null
}

export type UpdateBackupScheduleRequest = Pick<
	BackupSchedule,
	'enabled' | 'interval_hours' | 'hour_utc' | 'keep_last'
>

export interface ServerMember {
	id: Ulid
	user: UserRef
	role: ServerRole
	permissions: PermissionMask
	joined_at: Rfc3339 | null
	invited_at: Rfc3339
	last_invite_sent: Rfc3339 | null
	invite_resend_available_at: Rfc3339 | null
	pending: boolean
	is_owner: boolean
}

export interface ServerMemberList {
	members: ServerMember[]
}

export interface AddMemberRequest {
	user_id: Ulid
	role: Exclude<ServerRole, 'owner'>
}

export interface UpdateMemberRequest {
	role: Exclude<ServerRole, 'owner'>
}

export interface ReinviteResponse {
	sent: boolean
	cooldown_seconds: number | null
	member: ServerMember
}

export interface Invitation {
	id: Ulid
	server: { id: Ulid; name: string }
	role: ServerRole
	invited_by: UserRef
	invited_at: Rfc3339
	last_invite_sent: Rfc3339 | null
}

export interface InvitationList {
	invitations: Invitation[]
}

export type AuditAction =
	| 'server_created'
	| 'server_reallocated'
	| 'server_repaired'
	| 'server_reset'
	| 'server_started'
	| 'server_stopped'
	| 'server_restarted'
	| 'server_killed'
	| 'console_cleared'
	| 'console_command_executed'
	| 'changed_server_name'
	| 'user_invited'
	| 'user_invite_revoked'
	| 'user_permission_modified'
	| 'user_removed'
	| 'addon_added'
	| 'addon_uploaded'
	| 'addon_disabled'
	| 'addon_enabled'
	| 'addon_deleted'
	| 'addon_updated'
	| 'modpack_changed'
	| 'modpack_unlinked'
	| 'port_allocation_added'
	| 'port_allocation_removed'
	| 'loader_version_edited'
	| 'game_version_edited'
	| 'server_properties_modified'
	| 'startup_command_modified'
	| 'java_runtime_modified'
	| 'java_version_modified'
	| 'file_uploaded'
	| 'file_deleted'
	| 'file_renamed'
	| 'file_edited'
	| 'backup_created'
	| 'backup_renamed'
	| 'backup_restored'
	| 'backup_deleted'

export interface AuditEntry {
	id: Ulid
	actor: { type: 'user'; user_id: Ulid }
	action: { action: AuditAction; metadata: Record<string, unknown> | null }
	server_id: Ulid
	world_id: null
	timestamp: Rfc3339
}

export interface AuditLogPage {
	next_offset: number | null
	data: AuditEntry[]
	users: Record<string, { username: string; avatar_url: string | null }>
	addons: Record<string, { title: string; slug: string | null; icon_url: string | null }>
	versions: Record<string, { name: string; version_number: string | null }>
}

export interface AuditLogQuery {
	limit?: number
	offset?: number
	order?: 'asc' | 'desc'
	min_datetime?: Rfc3339
	max_datetime?: Rfc3339
	actor?: Ulid[]
	action?: AuditAction[]
}

export interface LoginRequest {
	username: string
	password: string
}

export interface ChangePasswordRequest {
	current_password: string
	new_password: string
}

export interface UserSearchResponse {
	users: UserRef[]
}

export type SystemUserState = 'provisioning' | 'ready' | 'error'

export interface SystemUser {
	state: SystemUserState
	name: string
	uid: number | null
	error_message: string | null
}

export type CpuMode = 'cap' | 'share'

export interface UserLimits {
	memory_mib: number
	cpu_mode: CpuMode
	cpu_cores: number
	pids_max: number
	disk_mib: number
}

export interface MemoryUsage {
	limit_mib: number | null
	allocated_mib: number
	used_bytes: number
}

export interface CpuUsage {
	limit_cores: number | null
	used_cores: number
}

export interface DiskUsage {
	limit_mib: number | null
	used_bytes: number
	servers_bytes: number
	backups_bytes: number
	complete: boolean
}

export type LimitDimension = 'memory' | 'cpu' | 'pids' | 'disk'

export interface UserUsage {
	memory: MemoryUsage
	cpu: CpuUsage
	pids: { limit: number | null; used: number }
	disk: DiskUsage
	servers: { total: number; running: number }
	over_limit: boolean
	over_limit_dimensions: LimitDimension[]
	measured_at: Rfc3339
}

export type BlockedReason = 'over_limit' | 'system_user_not_ready' | null

export interface Capabilities {
	can_create_servers: boolean
	can_start_servers: boolean
	can_manage_panel_users: boolean
	blocked_reason: BlockedReason
}

export type AccountOrigin = 'admin' | 'registration'

export interface PanelUser {
	id: Ulid
	username: string
	avatar_url: string | null
	panel_role: PanelRole
	email: string | null
	origin: AccountOrigin
	created_at: Rfc3339
	last_login_at: Rfc3339 | null
	must_change_password: boolean
	system_user: SystemUser
	limits: UserLimits | null
	usage: UserUsage
}

export interface Me extends PanelUser {
	capabilities: Capabilities
	session: { id: Ulid; expires_at: Rfc3339 }
}

export interface OwnedServerRef {
	id: Ulid
	name: string
	memory_mib: number
	running: boolean
}

export interface AdminUserDetail extends PanelUser {
	owned_servers: OwnedServerRef[]
	active_sessions: number
}

export interface AdminUserList {
	users: PanelUser[]
	total: number
}

export interface CreateUserRequest {
	username: string
	password: string
	panel_role: PanelRole
	email?: string | null
	must_change_password?: boolean
	limits?: UserLimits
}

export interface UpdateUserRequest {
	username?: string
	panel_role?: PanelRole
	password?: string
	email?: string | null
	must_change_password?: boolean
}

export type DeleteUserServers = 'delete' | 'transfer'

export interface UserLimitsResponse {
	limits: UserLimits | null
	usage: UserUsage
	host: { cpu_cores: number; assignable_memory_mib: number; assignable_disk_mib: number }
}

export interface HostCapacity {
	cpu_cores: number
	memory_total_bytes: number
	reserved_memory_mib: number
	assignable_memory_mib: number
	disk_total_bytes: number
	assignable_disk_mib: number
	allocated: { memory_mib: number; cpu_cores: number; disk_mib: number }
	used: { memory_bytes: number; cpu_cores: number; pids: number }
	user_count: number
	unlimited_users: number
	default_limits: UserLimits
	measured_at: Rfc3339
}

export interface PanelSettings {
	public_address: string | null
	port_pool: { from: number; to: number }
	default_limits: UserLimits
	max_upload_bytes: number
	max_backups_per_server: number
	external_services_enabled: boolean
	max_concurrent_operations: number
	stop_grace_seconds: number
	registration_enabled: boolean
	registration_requires_approval: boolean
	java_auto_install: boolean
}

export interface WsServerMessage {
	type: 'server'
	server: Server
}

export interface WsStateMessage {
	type: 'state'
	power_state: PowerState
	target: PowerTarget | null
	uptime_seconds: number
	exit_code: number | null
	oom_killed: boolean
}

export interface WsStatsMessage {
	type: 'stats'
	cpu_percent: number
	ram_usage_bytes: number
	ram_total_bytes: number
	storage_usage_bytes: number
	storage_total_bytes: number
}

export interface WsOperationsMessage {
	type: 'operations'
	revision: number
	busy_reasons: BusyReasonCode[]
	operations: Operation[]
}

export interface WsConsoleHistoryStartMessage {
	type: 'console_history_start'
	total_lines: number
	dropped_lines: number
}

export interface WsConsoleMessage {
	type: 'console'
	seq: number
	lines: string[]
}

export interface WsConsoleHistoryEndMessage {
	type: 'console_history_end'
}

export interface WsConsoleClearedMessage {
	type: 'console_cleared'
}

export interface WsContentChangedMessage {
	type: 'content_changed'
	reason: 'updates_checked' | 'external_change'
}

export interface WsBackupListChangedMessage {
	type: 'backup_list_changed'
}

export interface WsStartupChangedMessage {
	type: 'startup_changed'
	java_version: number | null
	jre_vendor: JreVendor | null
	memory_mib: number
	startup_command: string
	original_invocation: string
	restart_required: boolean
}

export interface WsNetworkChangedMessage {
	type: 'network_changed'
	primary_port: number
	allocations: Allocation[]
}

export type WsMessage =
	| WsServerMessage
	| WsStateMessage
	| WsStatsMessage
	| WsOperationsMessage
	| WsConsoleHistoryStartMessage
	| WsConsoleMessage
	| WsConsoleHistoryEndMessage
	| WsConsoleClearedMessage
	| WsContentChangedMessage
	| WsBackupListChangedMessage
	| WsStartupChangedMessage
	| WsNetworkChangedMessage

export type WsClientMessage = never
