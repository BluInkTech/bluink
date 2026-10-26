/**
 * Recognises a failure to listen because another process holds the port, and
 * explains how to stop it. Each platform reports the conflict differently.
 */
const portInUse = [
	/os error 10048\b/, // Windows: WSAEADDRINUSE
	/os error 98\b/, // Linux: EADDRINUSE
	/os error 48\b/, // macOS: EADDRINUSE
	/address already in use/i,
]

/** The command that stops whatever listens on `port`, for `platform`. */
export function stopListenerCommand(port: number, platform: NodeJS.Platform) {
	return platform === 'win32'
		? `Get-NetTCPConnection -LocalPort ${port} -State Listen | ForEach-Object { Stop-Process -Id $_.OwningProcess }`
		: `lsof -ti tcp:${port} -sTCP:LISTEN | xargs kill`
}

/** A clearer error when `error` means `port` is taken; otherwise `error` itself. */
export function explainPortInUse(
	error: unknown,
	port: number,
	platform: NodeJS.Platform = process.platform,
) {
	if (
		!(error instanceof Error) ||
		!portInUse.some((pattern) => pattern.test(error.message))
	)
		return error
	const shell = platform === 'win32' ? 'PowerShell' : 'a terminal'
	return new Error(
		`Port ${port} is already in use, probably by an earlier Bluink server.\n` +
			`Stop it, then start again. In ${shell}, run:\n` +
			`  ${stopListenerCommand(port, platform)}\n` +
			`Or choose another port with --port.`,
		{ cause: error },
	)
}
