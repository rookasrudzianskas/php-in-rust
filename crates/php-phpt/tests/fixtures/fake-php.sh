#!/bin/sh
# A fake "php" binary for the php-run-tests integration tests.
#
# It understands just enough of the php CLI to be driven by run-tests:
#   -m                    prints a module list
#   -f <script> / <last>  the "script" to run
#   -d name=value         recorded (see the @ini directive)
#   -- args...            script arguments (see the @args directive)
#
# The script is NOT PHP: every line is echoed to stdout, except for
# directives that start with '@':
#   @sleep N          sleep N seconds
#   @stderr TEXT      print TEXT on stderr
#   @env NAME         print NAME=<value of environment variable> (or <unset>)
#   @args             print every argument after '--' as arg:<value>
#   @stdin            copy stdin to stdout
#   @ini NAME         print NAME=<value passed with -d> (or <unset>)
#   @exit N           exit with status N
#   @kill             die from SIGSEGV
#   @touch FILE       create FILE
#   @count FILE       increment the number stored in FILE and print "run N"
#   @pwd              print the physical working directory
#   @bytes            print the bytes 'a', 0xFF, 'b' and a newline
#   @crlf             print "one" CRLF "two" CRLF
#   @leak             start a background process that keeps stdout open
#   @lock DIR         mkdir DIR, printing VIOLATION if it already exists
#   @unlock DIR       rmdir DIR

for a in "$@"; do
    if [ "$a" = "-m" ]; then
        printf '[PHP Modules]\nCore\njson\nstandard\n\n[Zend Modules]\nZend OPcache\n'
        exit 0
    fi
done

script=""
prev=""
INI_LIST=""
for a in "$@"; do
    if [ "$prev" = "-f" ]; then script="$a"; fi
    if [ "$prev" = "-d" ]; then
        INI_LIST="$INI_LIST
$a"
    fi
    prev="$a"
done
if [ -z "$script" ]; then
    for a in "$@"; do script="$a"; done
fi

# The script is read on fd 3 so that stdin stays available for @stdin.
while IFS= read -r line <&3 || [ -n "$line" ]; do
    case "$line" in
        "@sleep "*) sleep "${line#@sleep }" ;;
        "@stderr "*) printf '%s\n' "${line#@stderr }" >&2 ;;
        "@env "*)
            name="${line#@env }"
            eval "val=\${$name-<unset>}"
            printf '%s=%s\n' "$name" "$val"
            ;;
        "@args")
            seen=0
            for a in "$@"; do
                if [ "$seen" = 1 ]; then printf 'arg:%s\n' "$a"; fi
                if [ "$a" = "--" ]; then seen=1; fi
            done
            ;;
        "@stdin") cat ;;
        "@ini "*)
            name="${line#@ini }"
            val=$(printf '%s\n' "$INI_LIST" | grep "^$name=" | tail -1)
            if [ -z "$val" ]; then val="$name=<unset>"; fi
            printf '%s\n' "$val"
            ;;
        "@exit "*) exit "${line#@exit }" ;;
        "@kill") kill -SEGV $$ ;;
        "@touch "*) : > "${line#@touch }" ;;
        "@count "*)
            f="${line#@count }"
            n=$(cat "$f" 2>/dev/null || echo 0)
            n=$((n + 1))
            echo "$n" > "$f"
            echo "run $n"
            ;;
        "@pwd") pwd -P ;;
        "@bytes") printf 'a\377b\n' ;;
        "@crlf") printf 'one\r\ntwo\r\n' ;;
        "@leak") (sleep 30 &) ;;
        "@lock "*) mkdir "${line#@lock }" 2>/dev/null || echo VIOLATION ;;
        "@unlock "*) rmdir "${line#@unlock }" ;;
        *) printf '%s\n' "$line" ;;
    esac
done 3< "$script"
