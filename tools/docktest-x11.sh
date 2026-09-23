#!/bin/bash
# Docking test on an X11 display: for each edge, move NativeTerm's window
# there, move the pointer away (the window should hide behind a strip),
# touch the strip (it should come back). Expects DISPLAY/XAUTHORITY set and
# ~/nt-target/debug/nativeterm built. Prints one line per step.
# the frame's top-left and the window's size (xwininfo, minus _NET_FRAME_EXTENTS)
geo() { local x y w h e; read x y w h <<< "$(xwininfo -id "$1" 2>/dev/null | awk '/Absolute upper-left X/{x=$4} /Absolute upper-left Y/{y=$4} /Width/{w=$2} /Height/{h=$2} END{print x, y, w, h}')"
  e=($(xprop -id "$1" _NET_FRAME_EXTENTS 2>/dev/null | grep -o "[0-9, ]*$" | sed 's/,//g')); echo "$((x - ${e[0]:-0})),$((y - ${e[2]:-0})) ${w}x$h"; }
mapped() { xwininfo -id "$1" 2>/dev/null | awk '/Map State/{print $3}'; }
pkill -x nativeterm; sleep 1
rm -rf /tmp/nt-dock
nohup ~/nt-target/debug/nativeterm --data-dir /tmp/nt-dock --ssh-dir /tmp/nt-look-ssh > /tmp/nt-dock.log 2>&1 < /dev/null &
sleep 7
M=""
for w in $(xdotool search --name "^NativeTerm" 2>/dev/null); do
  [ "$(xdotool getwindowgeometry $w | awk '/Geometry/{print $2}')" = "960x640" ] && M=$w
done
echo "main=$M"
[ -z "$M" ] && { tail -3 /tmp/nt-dock.log; exit 1; }
read WX WY WW WH <<< "$(xprop -root _NET_WORKAREA | sed 's/.*= //; s/,//g' | awk '{print $1, $2, $3, $4}')"
echo "workarea $WX,$WY ${WW}x$WH"
CX=$((WX + WW / 2)); CY=$((WY + WH / 2))
# the frame's top-left, whatever the window manager takes a move to mean
frame() { local x y; read x y <<< "$(xwininfo -id $M | awk '/Absolute upper-left X/{x=$4} /Absolute upper-left Y/{y=$4} END{print x, y}')"
  local e; e=($(xprop -id $M _NET_FRAME_EXTENTS 2>/dev/null | sed 's/.*= //; s/,//g')); echo $((x - ${e[0]:-0})) $((y - ${e[2]:-0})); }
moveframe() { xdotool windowmove $M $1 $2; sleep 0.5; local fx fy; read fx fy <<< "$(frame)"; [ "$fx,$fy" != "$1,$2" ] && xdotool windowmove $M $(( $1 + $1 - fx )) $(( $2 + $2 - fy )); sleep 0.3; }
EXT=($(xprop -id $M _NET_FRAME_EXTENTS 2>/dev/null | sed 's/.*= //; s/,//g'))
FW=$((960 + ${EXT[0]:-0} + ${EXT[1]:-0}))
strip() { for w in $(xdotool search --name "^NativeTerm" 2>/dev/null); do g=$(xdotool getwindowgeometry $w | awk '/Geometry/{print $2}'); case $g in *x4|4x*) echo $w;; esac; done | head -1; }
for edge in top left right; do
  xdotool mousemove $CX $CY; sleep 0.5
  xdotool windowactivate --sync $M 2>/dev/null
  case $edge in
    top) TX=$((WX + 200)); TY=$WY ;;
    left) TX=$WX; TY=$((WY + 150)) ;;
    right) TX=$((WX + WW - FW)); TY=$((WY + 150)) ;;
  esac
  moveframe $TX $TY
  sleep 0.3; xdotool mousemove $CX $((CY - 50)); sleep 2.5
  docked="$(geo $M)"
  # pointer far away from the window
  case $edge in
    top) xdotool mousemove $CX $((WY + WH - 20)) ;;
    left) xdotool mousemove $((WX + WW - 20)) $CY ;;
    right) xdotool mousemove $((WX + 20)) $CY ;;
  esac
  sleep 2.5
  S=$(strip)
  hidden="main=$(mapped $M) strip=$( [ -n "$S" ] && echo "$(mapped $S) $(geo $S)" )"
  # touch the strip
  case $edge in
    top) xdotool mousemove $((WX + 200 + 480)) $((WY + 1)) ;;
    left) xdotool mousemove $((WX + 1)) $((WY + 150 + 320)) ;;
    right) xdotool mousemove $((WX + WW - 2)) $((WY + 150 + 320)) ;;
  esac
  sleep 0.4; xdotool mousemove_relative 1 1; sleep 2
  back="main=$(mapped $M) $(geo $M)"
  echo "$edge | docked: $docked | hidden: $hidden | back: $back"
  # undock: move the window to the middle
  xdotool mousemove $CX $CY; sleep 0.3
  xdotool windowmove $M $((CX - 480)) $((CY - 320)); sleep 2.5
done
pkill -x nativeterm
