const PRESENTER_INSTALL_SCRIPT =
  'https://raw.githubusercontent.com/church-helpers-hungary/metocast/master/presenter-receiver/install.sh';

/** One-line installer for the presenter receiver; `--service` turns on console auto-login. */
export function presenterInstallCommand(wsUrl: string, autoStart: boolean): string {
  return `curl -fsSL ${PRESENTER_INSTALL_SCRIPT} | bash -s -- ${wsUrl}${autoStart ? ' --service' : ''}`;
}
