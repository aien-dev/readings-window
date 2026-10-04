#!/bin/bash
# exposure-watch: what the house shows on its public IP, scanned from INSIDE the LAN (router hairpin view). Scans ONLY our own public IP.
# Inside view != internet view: the router answers its own 53/80 and reflects LAN-only forwards (e.g. 9443). Baseline holds the accepted inside view; Ports in optional_tcp may come and go without alarming (the router's own 53/80 flicker in the hairpin view).
# the real outside view is a separate, occasional check (check-host.net or similar). A change here means "something on the router or LAN moved", not proof of exposure.
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
OUT=/var/lib/readings-window/exposure.json
STATE=/var/lib/exposure-watch
BASE=/etc/exposure-watch/baseline.json
mkdir -p "$STATE" /etc/exposure-watch
RUNLOG=$STATE/last-run.txt
log() { echo "$*" >> "$RUNLOG.new"; }
: > "$RUNLOG.new"
ts=$(date +%s)
log "start $(date -u +%FT%TZ)"

# a. public IP
ip=$(curl -s -m 10 https://api.ipify.org)
if ! [[ $ip =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  ip=""
  command -v dig >/dev/null && ip=$(dig +short +time=5 +tries=1 myip.opendns.com @resolver1.opendns.com | head -1)
  [[ $ip =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || ip=""
fi
if [ -z "$ip" ]; then
  log "FAIL could not learn public IP; no result written"
  mv "$RUNLOG.new" "$RUNLOG"; exit 1
fi

# baseline (first run creates it from the expected lists)
if [ ! -s "$BASE" ]; then
  echo '{"vantage":"lan-hairpin","expected_tcp":[443,9443],"optional_tcp":[53,80],"expected_udp":[3478],"note":"inside view; outside check 2026-10-04 (check-host.net) showed only 443/tcp"}' > "$BASE"
  log "baseline created"
fi
exp_tcp=$(grep -o '"expected_tcp":\[[^]]*\]' "$BASE" | grep -o '[0-9]\+' | sort -n | tr '\n' ' ')
exp_udp=$(grep -o '"expected_udp":\[[^]]*\]' "$BASE" | grep -o '[0-9]\+' | sort -n | tr '\n' ' ')
opt_tcp=$(grep -o '"optional_tcp":\[[^]]*\]' "$BASE" | grep -o '[0-9]\+' | sort -n | tr '\n' ' ')
base_note=$(grep -o '"note":"[^"]*"' "$BASE" | sed 's/^"note":"//; s/"$//')

# b. scans
PORTS="1-1024,1194,1723,2222,3000,3010,3389,4433,5000,5900,8000,8080,8443,9443,10443,18443,41641,50443"
open_tcp=""; open_udp=""; udp_note=""; scan_ok=1
if command -v nmap >/dev/null; then
  res=$(nmap -Pn -n -sT --open -T4 --max-retries 1 --host-timeout 300s -p "$PORTS" -oG - "$ip" 2>/dev/null)
  echo "$res" | grep -q '^Host:' || echo "$res" | grep -q 'Status: Up\|Ports:' || scan_ok=0
  open_tcp=$(echo "$res" | grep -o '[0-9]\+/open/tcp' | cut -d/ -f1 | sort -n | tr '\n' ' ')
  ures=$(nmap -Pn -n -sU --open -p 3478,41641 --max-retries 2 -oG - "$ip" 2>/dev/null)
  open_udp=$(echo "$ures" | grep -o '[0-9]\+/open/udp' | cut -d/ -f1 | sort -n | tr '\n' ' ')
  udp_note="UDP open/filtered is not distinguishable; only 'open' counted."
else
  udp_note="UNVERIFIED: nmap missing, UDP not checked."
  conn() { timeout 2 bash -c "exec 3<>/dev/tcp/$ip/$1" 2>/dev/null && echo "$1"; }
  list=$(seq 1 1024; for p in 1194 1723 2222 3000 3010 3389 4433 5000 5900 8000 8080 8443 9443 10443 18443 41641 50443; do echo $p; done)
  open_tcp=$(echo "$list" | xargs -P 64 -I{} bash -c "timeout 2 bash -c 'exec 3<>/dev/tcp/$ip/{}' 2>/dev/null && echo {}" | sort -n | tr '\n' ' ')
fi
log "scan_ok=$scan_ok tcp=[$open_tcp] udp=[$open_udp] $udp_note"

# c. UPnP
upnp=off; mappings=""
up=$(timeout 30 upnpc -l 2>&1)
if command -v upnpc >/dev/null; then
  if echo "$up" | grep -qi 'No IGD UPnP Device found\|No valid UPNP Internet Gateway\|upnpDiscover() error'; then upnp=off
  elif echo "$up" | grep -qi 'Found valid IGD\|Found a valid IGD\|List of UPNP port mappings'; then upnp=on
  fi
  mappings=$(echo "$up" | grep -E '^ *[0-9]+ +(TCP|UDP) ' | sed 's/^ *//; s/"/\\"/g')
else
  log "UNVERIFIED: upnpc missing"
fi
log "upnp=$upnp"

# d. DNS
dns_ok=true; dns_detail=""
for h in hs.aienos.com readings.aienos.com; do
  a=$(dig +short +time=5 +tries=1 A "$h" 2>/dev/null | grep -E '^[0-9.]+$' | sort | tr '\n' ' ')
  if [ "$a" != "$ip " ]; then dns_ok=false; dns_detail="$dns_detail $h=${a:-none}"; fi
done

# e. compare
changes=()
fmt() { echo "$*" | xargs; }
for p in $open_tcp; do [[ " $exp_tcp" == *" $p "* || " $opt_tcp" == *" $p "* ]] || changes+=("TCP port $p is open in the inside view and is not in the baseline."); done
for p in $exp_tcp;  do [[ " $open_tcp" == *" $p "* ]] || changes+=("Baseline TCP port $p is not reachable in the inside view."); done
for p in $open_udp; do [[ " $exp_udp" == *" $p "* ]] || changes+=("UDP port $p is open in the inside view and is not in the baseline."); done
[ "$upnp" = on ] && changes+=("The router answers UPnP, so devices could open ports by themselves.")
[ -n "$mappings" ] && changes+=("The router lists UPnP port mappings.")
[ "$dns_ok" = false ] && changes+=("DNS does not match the public IP $ip:$dns_detail.")
[ "$scan_ok" = 0 ] && changes+=("The port scan did not complete; result UNVERIFIED.")
status=ok; [ ${#changes[@]} -gt 0 ] && status=changed

jarr() { local o="" x; for x in "$@"; do o="$o${o:+,}$x"; done; echo "[$o]"; }
sarr() { local o="" x; for x in "$@"; do o="$o${o:+,}\"$x\""; done; echo "[$o]"; }
mapjson=$(printf '%s\n' "$mappings" | sed '/^$/d' | sed 's/^/"/; s/$/"/' | paste -sd, -)
chjson=""; for c in "${changes[@]}"; do chjson="$chjson${chjson:+,}\"${c//\"/\\\"}\""; done
json=$(printf '{"ts":%s,"vantage":"lan-hairpin","baseline_note":"%s","public_ip":"%s","open_tcp":%s,"open_udp":%s,"upnp":"%s","upnp_mappings":[%s],"dns_ok":%s,"expected_tcp":%s,"optional_tcp":%s,"expected_udp":%s,"status":"%s","changes":[%s]}' \
  "$ts" "${base_note//\"/}" "$ip" "$(jarr $open_tcp)" "$(jarr $open_udp)" "$upnp" "$mapjson" "$dns_ok" "$(jarr $exp_tcp)" "$(jarr $opt_tcp)" "$(jarr $exp_udp)" "$status" "$chjson")
echo "$json" > "$OUT.tmp" && chmod 644 "$OUT.tmp" && mv "$OUT.tmp" "$OUT"
[ "$status" = changed ] && echo "$json" >> "$STATE/alerts.log"
log "status=$status end $(date -u +%FT%TZ)"
mv "$RUNLOG.new" "$RUNLOG"
