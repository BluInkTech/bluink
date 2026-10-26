import { mkdir, readFile } from 'node:fs/promises'
import { parseArgs } from 'node:util'
import path from 'node:path'
import os from 'node:os'
import { fileURLToPath } from 'node:url'
import {
	nativeBuildInfo,
	nativeSourceDigest,
	npm,
	root,
	run,
	targets,
	writeJson,
} from './release-utils.mjs'

export async function buildNative(
	targetName,
	{ mode = 'release', outputDirectory } = {},
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
	const sourceDigest = await nativeSourceDigest()
	const version = JSON.parse(
		await readFile(path.join(root, 'backend/package.json'), 'utf8'),
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
	env.CARGO_ENCODED_RUSTFLAGS = [
		inherited,
		`--remap-path-prefix=${root}=/bluink`,
		`--remap-path-prefix=${process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo')}=/cargo`,
		`--remap-path-prefix=${process.env.RUSTUP_HOME ?? path.join(os.homedir(), '.rustup')}=/rust`,
	]
		.filter(Boolean)
		.join('\u001f')
	const directory = outputDirectory
		? path.resolve(outputDirectory)
		: path.join(
				root,
				'backend/runtime',
				mode === 'development' ? 'development' : '',
			)
	await npm(
		[
			'exec',
			'--workspace',
			'backend',
			'--',
			'napi',
			'build',
			'--platform',
			'--profile',
			mode,
			'--manifest-path',
			'Cargo.toml',
			'--package-json-path',
			'package.json',
			'--output-dir',
			directory,
			'--no-js',
			'--target',
			target.rust,
		],
		{ env },
	)
	if (sourceDigest !== (await nativeSourceDigest()))
		throw new Error(
			'Native sources changed while compiling. Run the build again.',
		)
	const artifact = path.join(directory, target.artifact)
	const compiled = nativeBuildInfo(await readFile(artifact))
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
