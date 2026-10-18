#!/usr/bin/env bash
# 서버 한 대를 Tolkmeet용으로 맞춘다. provision.sh가 root로 실행하며, 여러 번 실행해도 결과가 같다.
# 비밀값은 다루지 않는다: secrets.env는 secrets.sh가, turn.env는 처음 한 번 여기서 서버 안에서 만든다.
set -euo pipefail
SRC=$(cd "$(dirname "$0")" && pwd)
export DEBIAN_FRONTEND=noninteractive

# ── 패키지 ─────────────────────────────────────────────
if [ ! -f /etc/apt/sources.list.d/caddy-stable.list ]; then
	apt-get update -q
	apt-get install -y -q debian-keyring debian-archive-keyring apt-transport-https curl gnupg
	curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/gpg.key | gpg --dearmor --yes -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
	curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt -o /etc/apt/sources.list.d/caddy-stable.list
fi
apt-get update -q
apt-get install -y -q caddy coturn curl openssl unattended-upgrades
if [ -n "${LITESTREAM_URL:-}" ] && ! dpkg -s litestream >/dev/null 2>&1; then
	curl -fsSL -o /tmp/litestream.deb "$LITESTREAM_URL"
	echo "${LITESTREAM_SHA256}  /tmp/litestream.deb" | sha256sum -c -
	dpkg -i /tmp/litestream.deb
	rm -f /tmp/litestream.deb
fi

# ── 사용자·폴더·설정 ───────────────────────────────────
id tolkmeet >/dev/null 2>&1 || useradd --system --home-dir /var/lib/tolkmeet --shell /usr/sbin/nologin tolkmeet
install -d -m 0755 /opt/tolkmeet /opt/tolkmeet/releases /opt/tolkmeet/bin
install -d -m 0750 -o root -g tolkmeet /etc/tolkmeet
install -m 0644 "$SRC/app.env" /etc/tolkmeet/app.env
[ -f /etc/tolkmeet/secrets.env ] || install -m 0600 /dev/null /etc/tolkmeet/secrets.env
if [ ! -s /etc/tolkmeet/turn.env ]; then
	(umask 077; echo "TURN_SECRET=$(openssl rand -hex 32)" >/etc/tolkmeet/turn.env)
fi
chmod 0600 /etc/tolkmeet/secrets.env /etc/tolkmeet/turn.env
install -m 0755 "$SRC/system/tolkmeet-health.sh" /opt/tolkmeet/bin/tolkmeet-health
install -m 0755 "$SRC/install-release.sh" /opt/tolkmeet/bin/install-release

# coturn: 공유 비밀을 서버 안에서만 채운다.
secret=$(sed -n 's/^TURN_SECRET=//p' /etc/tolkmeet/turn.env)
sed "s/@TURN_SECRET@/${secret}/" "$SRC/turnserver.conf.in" >/etc/turnserver.conf.new
chown root:turnserver /etc/turnserver.conf.new
chmod 0640 /etc/turnserver.conf.new
mv /etc/turnserver.conf.new /etc/turnserver.conf

# ── systemd ────────────────────────────────────────────
install -m 0644 "$SRC/system/tolkmeet.service" "$SRC/system/tolkmeet-health.service" "$SRC/system/tolkmeet-health.timer" /etc/systemd/system/
for s in caddy coturn litestream; do
	install -d /etc/systemd/system/$s.service.d
	install -m 0644 "$SRC/system/restart-always.conf" /etc/systemd/system/$s.service.d/restart-always.conf
done

# Litestream: 백업 버킷과 키가 준비된 뒤에만 켠다(앱과 같은 사용자로 DB를 읽는다).
if [ -n "${LITESTREAM_BUCKET:-}" ] && [ -s /etc/tolkmeet/litestream.env ] && command -v litestream >/dev/null; then
	sed "s/@BUCKET@/${LITESTREAM_BUCKET}/" "$SRC/litestream.yml.in" >/etc/litestream.yml
	chown root:tolkmeet /etc/litestream.yml
	chmod 0640 /etc/litestream.yml
	chmod 0600 /etc/tolkmeet/litestream.env
	cat >/etc/systemd/system/litestream.service.d/tolkmeet.conf <<'UNIT'
[Unit]
After=tolkmeet.service
[Service]
User=tolkmeet
Group=tolkmeet
EnvironmentFile=/etc/tolkmeet/litestream.env
UNIT
	litestream_on=1
fi

# Caddy
install -m 0644 "$SRC/Caddyfile" /etc/caddy/Caddyfile
install -d -o caddy -g caddy /var/log/caddy

# ── 운영체제 자동 복구 ─────────────────────────────────
if [ ! -f /swapfile ]; then
	fallocate -l 1G /swapfile
	chmod 600 /swapfile
	mkswap /swapfile >/dev/null
	swapon /swapfile
	echo '/swapfile none swap sw 0 0' >>/etc/fstab
fi
install -m 0644 "$SRC/system/sysctl.conf" /etc/sysctl.d/90-tolkmeet.conf
sysctl --system >/dev/null
echo softdog >/etc/modules-load.d/softdog.conf
modprobe softdog
install -d /etc/systemd/system.conf.d /etc/systemd/journald.conf.d
install -m 0644 "$SRC/system/watchdog.conf" /etc/systemd/system.conf.d/90-tolkmeet-watchdog.conf
install -m 0644 "$SRC/system/journald.conf" /etc/systemd/journald.conf.d/90-tolkmeet.conf
install -m 0644 "$SRC/system/unattended-upgrades.conf" /etc/apt/apt.conf.d/52tolkmeet-upgrades

systemctl daemon-reexec
systemctl daemon-reload
systemctl restart systemd-journald
systemctl enable --now tolkmeet-health.timer
systemctl enable coturn caddy
systemctl restart coturn
systemctl restart caddy
systemctl enable tolkmeet
# 첫 배포 전에는 띄울 실행 파일이 없다.
[ -e /opt/tolkmeet/current ] && systemctl restart tolkmeet
if [ "${litestream_on:-0}" = 1 ]; then
	systemctl enable litestream
	systemctl restart litestream
fi
echo "setup done"
