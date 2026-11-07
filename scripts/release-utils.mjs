import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync } from 'node:fs'
import { readFile, readdir, stat, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

export const root = path.resolve(fileURLToPath(new URL('../', import.meta.url)))
export const packageName = 'bluink'
export const registry = 'https://registry.npmjs.org/'
export const targets = [
	{
		rust: 'x86_64-unknown-linux-gnu',
		platform: 'linux',
		arch: 'x64',
		artifact: 'bluink-linux-x64',
		addon: 'bluink-linux-x64-gnu.node',
		addonPackage: '@bluink/cli-linux-x64-gnu',
	},
	{
		rust: 'x86_64-pc-windows-msvc',
		platform: 'win32',
		arch: 'x64',
		artifact: 'bluink-win32-x64.exe',
		addon: 'bluink-win32-x64.node',
		addonPackage: '@bluink/cli-win32-x64',
	},
	{
		rust: 'aarch64-apple-darwin',
		platform: 'darwin',
		arch: 'arm64',
		artifact: 'bluink-darwin-arm64',
		addon: 'bluink-darwin-arm64.node',
		addonPackage: '@bluink/cli-darwin-arm64',
	},
]

export function run(
	command,
	args,
	{ cwd = root, env = process.env, capture = false, timeout = 0 } = {},
) {
	return new Promise((resolve, reject) => {
		let output = ''
		const child = spawn(command, args, {
			cwd,
			env,
			timeout,
			stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'inherit',
		})
		let errors = ''
		child.stdout?.on('data', (chunk) => {
			output += chunk
		})
		child.stderr?.on('data', (chunk) => {
			errors += chunk
		})
		child.once('error', reject)
		child.once('close', (code) =>
			code === 0 && !child.killed
				? resolve(output)
				: reject(
						new Error(
							`${command} ${args.join(' ')} ${child.killed ? `timed out after ${timeout} ms` : `failed (${code})`}. ${errors.trim()}`,
						),
					),
		)
	})
}

/** Use Windows' libarchive tar, regardless of Git/MSYS tools on PATH. */
export function tar(args, options) {
	const command =
		process.platform === 'win32'
			? path.join(
					process.env.SystemRoot ?? 'C:\\Windows',
					'System32',
					'tar.exe',
				)
			: 'tar'
	return run(command, args, options)
}

export function npm(args, options) {
	const cli = [
		process.env.npm_execpath,
		path.resolve(
			path.dirname(process.execPath),
			'node_modules/npm/bin/npm-cli.js',
		),
		path.resolve(
			path.dirname(process.execPath),
			'../lib/node_modules/npm/bin/npm-cli.js',
		),
	].find((file) => file && existsSync(file))
	if (!cli)
		throw new Error(
			'Run this command through npm so its CLI path is available.',
		)
	return run(process.execPath, [cli, ...args], options)
}

export async function json(file) {
	return JSON.parse(await readFile(file, 'utf8'))
}
export async function writeJson(file, value) {
	await writeFile(file, `${JSON.stringify(value, null, 2)}\n`)
}
export function sha256(bytes) {
	return createHash('sha256').update(bytes).digest('hex')
}
export async function fileHash(file) {
	return sha256(await readFile(file))
}

async function sourceFiles(folder) {
	const files = []
	for (const entry of await readdir(folder, { withFileTypes: true })) {
		const file = path.join(folder, entry.name)
		if (entry.isDirectory()) files.push(...(await sourceFiles(file)))
		else if (entry.isFile()) files.push(file)
		else throw new Error(`Native source must be a regular file: ${file}`)
	}
	return files
}

/** Hash the Rust workspace, engine/runtime sources and resolved dependencies. */
export async function nativeSourceDigest(workspaceRoot = root) {
	const files = [
		...(await sourceFiles(path.join(workspaceRoot, 'backend/src'))),
		...(await sourceFiles(path.join(workspaceRoot, 'backend/format/src'))),
		...(await sourceFiles(path.join(workspaceRoot, 'backend/addon/src'))),
		...(
			await sourceFiles(path.join(workspaceRoot, 'backend/resources'))
		).filter((file) => path.basename(file) !== 'workspace-starter.json'),
		...[
			'rust-toolchain.toml',
			'fonts/catalogue.json',
			'backend/Cargo.toml',
			'backend/format/Cargo.toml',
			'backend/addon/Cargo.toml',
			'backend/addon/build.rs',
			'backend/Cargo.lock',
			'backend/build.rs',
			'scripts/build-native.mjs',
			'scripts/release-identity.mjs',
			'scripts/release-utils.mjs',
		].map((file) => path.join(workspaceRoot, file)),
	]
	const hash = createHash('sha256')
	for (const file of files.sort()) {
		hash.update(path.relative(workspaceRoot, file).split(path.sep).join('/'))
		hash.update('\0')
		hash.update(await readFile(file))
		hash.update('\0')
	}
	return hash.digest('hex')
}

/** The executable also contains the bare marker strings (the build-info
 * command strips them), so accept only a marker that encloses a JSON object,
 * and only one distinct identity. */
export function nativeBuildInfo(bytes) {
	const prefix = Buffer.from('BLUINK_NATIVE_BUILD_INFO_V1:')
	const suffix = Buffer.from(':END_BLUINK_NATIVE_BUILD_INFO')
	const identities = new Set()
	for (
		let begin = bytes.indexOf(prefix);
		begin >= 0;
		begin = bytes.indexOf(prefix, begin + prefix.length)
	) {
		const end = bytes.indexOf(suffix, begin + prefix.length)
		if (end < 0) break
		const text = bytes.toString('utf8', begin + prefix.length, end)
		if (!text.startsWith('{')) continue
		try {
			JSON.parse(text)
			identities.add(text)
		} catch {
			// Not an embedded identity.
		}
	}
	if (identities.size > 1)
		throw new Error('Native artifact has more than one build identity.')
	if (!identities.size)
		throw new Error(
			'Native artifact has no build identity. Rebuild it with npm run build:native.',
		)
	return JSON.parse([...identities][0])
}

export function nativeHostTarget() {
	const target = targets.find(
		(value) =>
			value.platform === process.platform && value.arch === process.arch,
	)
	if (!target)
		throw new Error(
			`Unsupported native host: ${process.platform}/${process.arch}`,
		)
	return target
}

export function nativeDirectory(mode, workspaceRoot = root) {
	if (!['development', 'release'].includes(mode))
		throw new Error(
			`Unknown native build mode: ${mode}. Choose development or release.`,
		)
	return path.join(workspaceRoot, 'backend/dist/native', mode)
}

/** The verified host executable published by the native build. */
export function nativeExecutable(mode, workspaceRoot = root) {
	return path.join(
		nativeDirectory(mode, workspaceRoot),
		nativeHostTarget().artifact,
	)
}

/** npm runs the launcher with Node on every platform; it loads the addon from
 * the platform package npm installed as an optional dependency. */
export const packageLauncher = 'bin/bluink.mjs'
/** The addon's path inside each platform package. */
export const platformAddon = 'bluink.node'

export function nativeBuildCommand(mode) {
	return `npm run build:native:${mode === 'development' ? 'dev' : 'release'}`
}

/** Verify each released platform's addon. `locate` maps a target to its file;
 * by default the build output directory's `bluink-<platform>.node`. */
export async function verifyNativeArtifacts(
	directory,
	digest = undefined,
	hostOnly = false,
	identity = {},
	{ locate = (target) => path.join(directory, target.addon) } = {},
) {
	const expected = digest ?? (await nativeSourceDigest())
	return Promise.all(
		targets
			.filter(
				(target) =>
					!hostOnly ||
					(target.platform === process.platform &&
						target.arch === process.arch),
			)
			.map(async (target) => {
				const file = locate(target)
				const bytes = await readFile(file).catch(() => {
					throw new Error(
						`Missing ${path.relative(directory, file) || file}; build or copy ${target.rust} from the same native sources.`,
					)
				})
				const info = nativeBuildInfo(bytes)
				if (info.mode !== 'release')
					throw new Error(
						`Native artifact is not a release build: ${target.addon}. Run npm run build:native:release.`,
					)
				if (
					Object.keys(info).some(
						(key) =>
							![
								'sourceDigest',
								'target',
								'version',
								'publicCommit',
								'mode',
							].includes(key),
					) ||
					(identity.version && info.version !== identity.version) ||
					(identity.publicCommit && info.publicCommit !== identity.publicCommit)
				)
					throw new Error(`Native release identity mismatch: ${target.addon}`)
				if (info.sourceDigest !== expected || info.target !== target.rust)
					throw new Error(
						`Stale or mismatched ${target.addon}. Rebuild ${target.rust} from the current native sources.`,
					)
				return {
					...info,
					file: target.addon,
					package: target.addonPackage,
					sha256: sha256(bytes),
					bytes: (await stat(file)).size,
				}
			}),
	)
}
