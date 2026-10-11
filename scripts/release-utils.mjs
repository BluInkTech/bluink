import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync } from 'node:fs'
import { readFile, readdir, stat, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

export const root = path.resolve(fileURLToPath(new URL('../', import.meta.url)))
export const packageName = '@bluink/cli'
export const registry = 'https://registry.npmjs.org/'
export const targets = [
	{
		rust: 'x86_64-unknown-linux-gnu',
		platform: 'linux',
		arch: 'x64',
		artifact: 'bluink_runtime.linux-x64-gnu.node',
		typst: 'linux-x64/typst',
	},
	{
		rust: 'x86_64-pc-windows-msvc',
		platform: 'win32',
		arch: 'x64',
		artifact: 'bluink_runtime.win32-x64-msvc.node',
		typst: 'windows-x64/typst.exe',
	},
	{
		rust: 'aarch64-apple-darwin',
		platform: 'darwin',
		arch: 'arm64',
		artifact: 'bluink_runtime.darwin-arm64.node',
		typst: 'macos-arm64/typst',
	},
]

export function run(
	command,
	args,
	{ cwd = root, env = process.env, capture = false } = {},
) {
	return new Promise((resolve, reject) => {
		let output = ''
		const child = spawn(command, args, {
			cwd,
			env,
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
			code === 0
				? resolve(output)
				: reject(
						new Error(
							`${command} ${args.join(' ')} failed (${code}). ${errors.trim()}`,
						),
					),
		)
	})
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
export async function nativeSourceDigest() {
	const files = [
		...(await sourceFiles(path.join(root, 'backend/runtime/src'))),
		...(await sourceFiles(path.join(root, 'backend/runtime/scripts'))),
		...(await sourceFiles(path.join(root, 'backend/vendor'))),
		...[
			'rust-toolchain.toml',
			'fonts/catalogue.json',
			'backend/Cargo.toml',
			'backend/Cargo.lock',
			'backend/runtime/build.rs',
			'scripts/build-native.mjs',
			'scripts/release-utils.mjs',
		].map((file) => path.join(root, file)),
	]
	const hash = createHash('sha256')
	for (const file of files.sort()) {
		hash.update(path.relative(root, file).split(path.sep).join('/'))
		hash.update('\0')
		hash.update(await readFile(file))
		hash.update('\0')
	}
	return hash.digest('hex')
}

export function nativeBuildInfo(bytes) {
	const prefix = Buffer.from('BLUINK_NATIVE_BUILD_INFO_V1:')
	const suffix = Buffer.from(':END_BLUINK_NATIVE_BUILD_INFO')
	const begin = bytes.indexOf(prefix)
	const end = bytes.indexOf(suffix, begin + prefix.length)
	if (begin < 0 || end < 0)
		throw new Error(
			'Native artifact has no build identity. Rebuild it with npm run build:native.',
		)
	return JSON.parse(bytes.toString('utf8', begin + prefix.length, end))
}

export async function verifyNativeArtifacts(
	directory,
	digest = undefined,
	hostOnly = false,
	identity = {},
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
				const file = path.join(directory, target.artifact)
				const bytes = await readFile(file).catch(() => {
					throw new Error(
						`Missing ${target.artifact}; build or copy this target from the same native sources.`,
					)
				})
				const info = nativeBuildInfo(bytes)
				if (
					Object.keys(info).some(
						(key) =>
							!['sourceDigest', 'target', 'version', 'publicCommit'].includes(
								key,
							),
					) ||
					(identity.version && info.version !== identity.version) ||
					(identity.publicCommit && info.publicCommit !== identity.publicCommit)
				)
					throw new Error(
						`Native release identity mismatch: ${target.artifact}`,
					)
				if (info.sourceDigest !== expected || info.target !== target.rust)
					throw new Error(
						`Stale or mismatched ${target.artifact}. Rebuild ${target.rust} from the current native sources.`,
					)
				return {
					...info,
					file: target.artifact,
					sha256: sha256(bytes),
					bytes: (await stat(file)).size,
				}
			}),
	)
}
