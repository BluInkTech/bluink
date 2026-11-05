import {
	mkdir,
	readFile,
	mkdtemp,
	rename,
	rm,
	writeFile,
} from 'node:fs/promises'
import { parseArgs } from 'node:util'
import path from 'node:path'
import os from 'node:os'
import { fileURLToPath } from 'node:url'
import {
	nativeBuildInfo,
	nativeDirectory,
	nativeBuildCommand,
	nativeSourceDigest,
	root,
	run,
	targets,
	writeJson,
} from './release-utils.mjs'

async function releaseBuildDrive(subst, drive, directory) {
	const mappings = (await run(subst, [], { capture: true })).split(/\r?\n/)
	const owned = `${drive}\\: => ${directory}`.toLowerCase()
	if (!mappings.some((mapping) => mapping.trim().toLowerCase() === owned))
		throw new Error(
			`Build drive ${drive} changed during compilation. Its current mapping was preserved.`,
		)
	await run(subst, [drive, '/D'], { capture: true })
}

/** Compile release C objects under a temporary drive, so the user's checkout
 * path never reaches them. SUBST refuses an occupied letter, including one won
 * by another build. */
export async function withWindowsBuildDrive(directory, compile) {
	await mkdir(directory, { recursive: true })
	const subst = path.join(process.env.SystemRoot, 'System32', 'subst.exe')
	for (const letter of 'ZYXWVUTSRQPONMLKJIHGFED') {
		const drive = `${letter}:`
		try {
			await run(subst, [drive, directory], { capture: true })
		} catch {
			continue
		}
		try {
			return await compile(`${drive}\\`)
		} finally {
			await releaseBuildDrive(subst, drive, directory)
		}
	}
	throw new Error(
		'No temporary Windows drive is available for a release build.',
	)
}

export async function buildNative(
	targetName,
	{
		mode = 'release',
		outputDirectory,
		workspaceRoot = root,
		command = (args, options) => run('cargo', args, options),
	} = {},
) {
	if (!['development', 'release'].includes(mode))
		throw new Error(
			`Unknown native build mode: ${mode}. Choose development or release.`,
		)
	if (mode === 'development' && process.env.BLUINK_PUBLIC_COMMIT)
		throw new Error('Public release builds must use release mode.')
	const started = performance.now()
	const target = targets.find((value) =>
		targetName
			? value.rust === targetName
			: value.platform === process.platform && value.arch === process.arch,
	)
	if (!target)
		throw new Error(
			`Unsupported native build target: ${targetName ?? `${process.platform}/${process.arch}`}`,
		)
	if (target.platform !== process.platform || target.arch !== process.arch)
		throw new Error(
			`Build ${target.rust} on its native platform using the matching CI runner. Local cross-compilation is not supported.`,
		)
	const directory = outputDirectory
		? path.resolve(outputDirectory)
		: nativeDirectory(mode, workspaceRoot)
	const sourceDigest = await nativeSourceDigest(workspaceRoot)
	const version = JSON.parse(
		await readFile(path.join(workspaceRoot, 'backend/package.json'), 'utf8'),
	).version
	const publicCommit = process.env.BLUINK_PUBLIC_COMMIT
	if (
		publicCommit &&
		publicCommit !==
			(await run('git', ['rev-parse', 'HEAD'], { capture: true })).trim()
	)
		throw new Error('Public build commit does not match this checkout.')
	const info = JSON.stringify({
		sourceDigest,
		target: target.rust,
		version,
		mode,
		...(publicCommit && { publicCommit }),
	})
	const env = { ...process.env, BLUINK_NATIVE_BUILD_INFO: info }
	const inherited =
		env.CARGO_ENCODED_RUSTFLAGS ??
		(env.RUSTFLAGS ?? '').split(/\s+/).filter(Boolean).join('\u001f')
	const targetDirectory = path.resolve(
		workspaceRoot,
		env.CARGO_TARGET_DIR ?? 'backend/target',
	)
	const cargoHome = env.CARGO_HOME ?? path.join(os.homedir(), '.cargo')
	env.CARGO_ENCODED_RUSTFLAGS = [
		inherited,
		...(mode === 'release'
			? [`--remap-path-prefix=${targetDirectory}=/build`]
			: []),
		`--remap-path-prefix=${root}=/bluink`,
		`--remap-path-prefix=${cargoHome}=/cargo`,
		`--remap-path-prefix=${process.env.RUSTUP_HOME ?? path.join(os.homedir(), '.rustup')}=/rust`,
	]
		.filter(Boolean)
		.join('\u001f')
	const args = [
		'build',
		'--locked',
		'--bin',
		'bluink',
		'--profile',
		mode,
		'--manifest-path',
		path.join(workspaceRoot, 'backend/Cargo.toml'),
		'--target',
		target.rust,
	]
	const compiledFile = (targetRoot) =>
		path.join(
			targetRoot,
			target.rust,
			mode,
			target.platform === 'win32' ? 'bluink.exe' : 'bluink',
		)
	if (mode === 'release' && process.platform === 'win32') {
		// Registry C sources are absolute in libgit2's __FILE__.
		env.CL = [
			env.CL,
			'/experimental:deterministic',
			`"/pathmap:${cargoHome}=/cargo"`,
		]
			.filter(Boolean)
			.join(' ')
		await withWindowsBuildDrive(targetDirectory, (drive) =>
			command(args, {
				env: { ...env, CARGO_TARGET_DIR: drive },
				cwd: workspaceRoot,
			}),
		)
	} else await command(args, { env, cwd: workspaceRoot })
	if (sourceDigest !== (await nativeSourceDigest(workspaceRoot)))
		throw new Error(
			'Native sources changed while compiling. Run the build again.',
		)
	const artifact = path.join(directory, target.artifact)
	const bytes = await readFile(compiledFile(targetDirectory))
	const compiled = nativeBuildInfo(bytes)
	if (
		compiled.sourceDigest !== sourceDigest ||
		compiled.target !== target.rust ||
		compiled.mode !== mode ||
		compiled.version !== version ||
		compiled.publicCommit !== publicCommit
	)
		throw new Error(
			'The compiler did not produce the requested native build identity.',
		)
	await mkdir(path.dirname(artifact), { recursive: true })
	// Publish the exact bytes whose embedded identity we just verified. Same-volume
	// rename keeps the previous artifact intact if compilation/validation fails.
	const publication = await mkdtemp(path.join(directory, '.native-publish-'))
	try {
		const candidate = path.join(publication, target.artifact)
		await writeFile(candidate, bytes, { mode: 0o755 })
		await rename(candidate, artifact)
	} catch (error) {
		throw new Error(
			`Could not publish the verified executable at ${artifact}. Stop any Bluink process using it and retry ${nativeBuildCommand(mode)}. Compilation output remains at ${compiledFile(targetDirectory)}. ${error.message}`,
			{ cause: error },
		)
	} finally {
		await rm(publication, { recursive: true, force: true })
	}
	await writeJson(`${artifact}.json`, compiled)
	const elapsedSeconds = (performance.now() - started) / 1000
	console.log(
		`Built ${target.artifact} (${mode}) from ${sourceDigest.slice(0, 12)} in ${elapsedSeconds.toFixed(2)}s`,
	)
	return { artifact, mode, elapsedSeconds }
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	const { values } = parseArgs({
		options: {
			target: { type: 'string' },
			mode: { type: 'string', default: 'release' },
			'output-dir': { type: 'string' },
		},
	})
	await buildNative(values.target, {
		mode: values.mode,
		outputDirectory: values['output-dir'],
	})
}
