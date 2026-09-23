#!/bin/bash
# Drag NativeTerm's window by its title bar to each edge with the mouse
# (as a person does, so the window manager's own edge tiling happens),
# release, then move the pointer away and touch the strip.
state() { local x y w h; read x y w h <<< "$(xwininfo -id "$1" 2>/dev/null | awk '/Absolute upper-left X/{x=$4} /Absolute upper-left Y/{y=$4} /Width/{w=$2} /Height/{h=$2} END{print x, y, w, h}')"
  echo "$x,$y ${w}x$h $(xwininfo -id "$1" | awk '/Map State/{print $3}') $(xprop -id "$1" _NET_WM_STATE | grep -o 'MAXIMIZED_[A-Z]*' | tr '\n' ' ')"; }
pkill -x nativeterm; sleep 1
rm -rf /tmp/nt-drag
NATIVETERM_DOCK_LOG=1 nohup ~/nt-target/debug/nativeterm --data-dir /tmp/nt-drag --ssh-dir /tmp/nt-look-ssh > /tmp/nt-drag.log 2>&1 < /dev/null &
sleep 7
M=""; for w in $(xdotool search --name "^NativeTerm" 2>/dev/null); do [ "$(xdotool getwindowgeometry $w | awk '/Geometry/{print $2}')" = "960x640" ] && M=$w; done
echo "main=$M"; [ -z "$M" ] && exit 1
read WX WY WW WH <<< "$(xprop -root _NET_WORKAREA | sed 's/.*= //; s/,//g' | awk '{print $1, $2, $3, $4}')"
CX=$((WX + WW / 2)); CY=$((WY + WH / 2))
drag() { # from the title bar to (x, y), slowly, then release
  local sx sy; read sx sy <<< "$(xwininfo -id $M | awk '/Absolute upper-left X/{x=$4} /Absolute upper-left Y/{y=$4} END{print x + 300, y - 20}')"
  xdotool mousemove $sx $sy; sleep 0.3; xdotool mousedown 1; sleep 0.3
  for i in 1 2 3 4 5 6 7 8 9 10; do xdotool mousemove $(( sx + ($1 - sx) * i / 10 )) $(( sy + ($2 - sy) * i / 10 )); sleep 0.08; done
  sleep 0.5; xdotool mouseup 1; sleep 3; }
for edge in ${EDGES:-left right top}; do
  xdotool windowactivate --sync $M 2>/dev/null
  case $edge in
    left) drag $WX $CY ;;
    right) drag $((WX + WW - 1)) $CY ;;
    top) drag $CX $WY ;;
  esac
  docked="$(state $M)"
  case $edge in
    left) xdotool mousemove $((WX + WW - 30)) $CY ;;
    right) xdotool mousemove $((WX + 30)) $CY ;;
    top) xdotool mousemove $CX $((WY + WH - 30)) ;;
  esac
  sleep 2.5; hidden="$(state $M)"
  case $edge in
    left) xdotool mousemove $((WX + 1)) $CY ;;
    right) xdotool mousemove $((WX + WW - 2)) $CY ;;
    top) xdotool mousemove $CX $((WY + 1)) ;;
  esac
  sleep 0.4; xdotool mousemove_relative 0 1; sleep 2
  echo "$edge | after drag: $docked | pointer away: $hidden | strip touched: $(state $M)"
  # drag back to the middle to undock
  drag $((CX - 200)) $((CY - 200)); sleep 1
done
pkill -x nativeterm
