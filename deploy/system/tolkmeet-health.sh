#!/bin/sh
# 서버가 살아 있는데 응답이 멈춘 경우: 1분마다 확인해 3번 연속 실패하면 다시 띄운다.
state=/run/tolkmeet-health
if curl -fsS --max-time 5 -o /dev/null http://127.0.0.1:8080/api/health; then
	echo 0 >"$state"
	exit 0
fi
n=$(( $(cat "$state" 2>/dev/null || echo 0) + 1 ))
echo "$n" >"$state"
if [ "$n" -ge 3 ]; then
	logger -t tolkmeet-health "health check failed $n times: restarting tolkmeet"
	systemctl restart tolkmeet
	echo 0 >"$state"
fi
