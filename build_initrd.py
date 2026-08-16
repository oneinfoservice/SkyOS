"""Build SkyOS initrd.tar with FHS directory structure."""
import tarfile, os, sys, io

# Binary manifest -- single home for the initrd contents. ade's app
# catalog resolves exec paths against these names; tests/test_app_wiring.py
# pins the wiring. mknod is deliberately excluded: the kernel mknod
# syscall is still reserved/not-implemented (kernel-owns-facility audit).
COREUTILS_BINS = [
        'ls', 'cat', 'mkdir', 'rm', 'cp', 'mv',
        'ps', 'clear', 'uname', 'printenv', 'sleep', 'yes',
        'rmdir', 'touch', 'hostname', 'which', 'env', 'echo', 'head', 'tail', 'wc', 'grep', 'ln', 'chmod',
        'printf', 'sort', 'uniq', 'uptime',
        'ping', 'nslookup', 'wget', 'ifconfig', 'netstat', 'telnet',
        'beep', 'dd', 'blkid', 'fdisk', 'df', 'du',
        'awk', 'basename', 'chown', 'cut', 'date', 'diff', 'dirname',
        'false', 'find', 'free', 'gzip', 'hexdump', 'id', 'kill',
        'lspci', 'mount', 'nc', 'nl', 'od', 'patch', 'readlink',
        'sed', 'stat', 'su', 'sync', 'tac', 'tar', 'tee', 'top', 'tr',
        'true', 'umount', 'whoami', 'xargs',
        # Rest of the built coreutils set; mknod deliberately excluded --
        # the kernel mknod syscall is still reserved/not-implemented.
        'base64', 'expr', 'fold', 'less', 'logname', 'md5sum',
        'mkfifo', 'mkfs_sargafs', 'more', 'nohup', 'seq', 'shuf',
        'split', 'stdbuf', 'sum', 'tsort', 'tty', 'users',
]

BINARIES = {
        'bin/init':          'init',
        'bin/sash':          'sash',
        'bin/svc':           'svc',
        'bin/vahid':         'vahid',
        'bin/login-manager': 'login-manager',
        'bin/login':         'login',
        'bin/passwd':        'passwd',
        'bin/skybuild':      'skybuild',
        'bin/setup':         'setup',
        'bin/sarga-term':    'sarga-term',
        'bin/sargaedit':     'sargaedit',
        'bin/calculator':    'calculator',
        'bin/clock':         'clock',
        'bin/calendar':      'calendar',
        'bin/notes':         'notes',
        'bin/paint':         'paint',
        'bin/search':        'search',
        'bin/tasks':         'tasks',
        'bin/archive':       'archive',
        'bin/sysinfo':       'sysinfo',
        'bin/sysmon':        'sysmon',
        'bin/ade':           'ade',
        'bin/skysettings':   'sargasettings',
        'bin/skyedit':       'sargaedit',
        'bin/skyfiles':      'sargafiles',
        'bin/sargaview':     'sargaview',
        'bin/aicli':         'aicli',
        'bin/skystore':      'skystore',
        'bin/spkg':          'spkg',
        'bin/httpd':         'httpd',
        'bin/wget':          'wget',
        'bin/curl':          'curl',
        'bin/dhcp-client':   'dhcp-client',
        'bin/resolve':       'resolve',
        'bin/ssh-server':    'ssh-server',
        'bin/echod':         'echod',
        'bin/udpechod':      'udpechod',
        'bin/udpechoc':      'udpechoc',
        'bin/futex_test':          'futex_test',
        'bin/sigchld_test':        'sigchld_test',
        'bin/sigint_test':         'sigint_test',
        'bin/sigalrm_test':        'sigalrm_test',
        'bin/pipe_signal_test':    'pipe_signal_test',
        'bin/perm_test':           'perm_test',
        'bin/dac_test':            'dac_test',
        'bin/ipc_echo':            'ipc_echo',
}

def build_initrd(root_dir: str, output_path: str):
    coreutils_bins = COREUTILS_BINS

    binaries = BINARIES
    for b in coreutils_bins:
        binaries[f'bin/{b}'] = b

    symlinks = {
        'sbin/init':          '../bin/init',
        'sbin/vahid':         '../bin/vahid',
        'sbin/svc':           '../bin/svc',
        'sbin/sargaedit':     '../bin/sargaedit',
    }

    empty_dirs = [
        'dev',
        'proc',
        'tmp',
        'usr/lib',
        'usr/share',
        'usr/include',
        'var/log',
        'var/cache',
        'var/spool',
        'var/spkg',
        'var/spkg/cache',
        'etc/spkg',
        'home/root',
        'mnt/cdrom',
        'mnt/usb',
    ]

    config_files = {
        'etc/init.toml': None,
        'etc/fstab': None,
        'etc/hostname': None,
        'etc/passwd': None,
        'etc/shadow': None,
        'etc/group': None,
        'etc/spkg/repos.conf': None,
    }

    if os.path.exists(output_path):
        os.remove(output_path)

    # Binary locations to search (from cargo build output)
    search_dirs = [
        os.path.join(root_dir, 'target', 'x86_64-sarga', 'release'),
        os.path.join(root_dir, 'target', 'x86_64-sarga', 'debug'),
        root_dir,
    ]

    with tarfile.open(output_path, 'w') as tar:
        # Add regular binaries
        for arcname, binary in binaries.items():
            found = False
            for d in search_dirs:
                full_path = os.path.join(d, binary)
                if os.path.exists(full_path):
                    tar.add(full_path, arcname=arcname)
                    print(f'  {arcname} ({os.path.getsize(full_path)} bytes)')
                    found = True
                    break
            if not found:
                print(f'  WARNING: {binary} not found in search paths')

        # Add config files
        init_toml_data = read_config(root_dir, 'etc/init.toml') or INIT_TOML_CONTENT
        config_data = {
            'etc/init.toml': init_toml_data,
            'etc/fstab': FSTAB_CONTENT,
            'etc/hostname': HOSTNAME_CONTENT,
            'etc/passwd': PASSWD_CONTENT,
            'etc/shadow': SHADOW_CONTENT,
            'etc/group': GROUP_CONTENT,
            'etc/spkg/repos.conf': REPOS_CONTENT,
        }
        for arcname, data in config_data.items():
            info = tarfile.TarInfo(name=arcname)
            info.type = tarfile.REGTYPE
            encoded = data.encode('utf-8')
            info.size = len(encoded)
            tar.addfile(info, io.BytesIO(encoded))
            print(f'  {arcname} ({len(encoded)} bytes)')

        # Add symlinks
        for arcname, target in symlinks.items():
            info = tarfile.TarInfo(name=arcname)
            info.type = tarfile.SYMTYPE
            info.linkname = target
            tar.addfile(info)
            print(f'  {arcname} -> {target}')

        # Add empty directories
        for dirname in empty_dirs:
            info = tarfile.TarInfo(name=dirname)
            info.type = tarfile.DIRTYPE
            info.mode = 0o755
            tar.addfile(info)
            print(f'  {dirname}/')

    size = os.path.getsize(output_path)
    print(f'\ninitrd.tar: {size} bytes ({size/1024:.1f} KB)')

def read_config(root_dir, path):
    full = os.path.join(root_dir, path)
    if os.path.exists(full):
        with open(full, 'r') as f:
            return f.read()
    return ''

FSTAB_CONTENT = """# /etc/fstab - filesystem mount table
# <source>  <mountpoint>  <fstype>  <options>  <dump>  <pass>
tmpfs       /tmp          tmpfs     defaults   0       0
tmpfs       /var/log      tmpfs     defaults   0       0
tmpfs       /var/cache    tmpfs     defaults   0       0
tmpfs       /home         tmpfs     defaults   0       0
"""

HOSTNAME_CONTENT = "skyos\n"

PASSWD_CONTENT = """root:x:0:0:root:/home/root:/bin/sash
"""

# Dev login: user "root", password "skyos" (PBKDF2-HMAC-SHA256, salt SKYOSDESKTOPSALT, 10000 iters; salt must be 16 bytes for libsarga verify_password).
SHADOW_CONTENT = """root:PBKDF2-534b594f534445534b544f5053414c54:49a40924d5952ca3cb66bc0200f2be5e063bf251704965667f02d8ee2b3ef252:10000:12000:0:99999:7:::
"""

GROUP_CONTENT = """root:x:0:root
wheel:x:1:root
users:x:100:
"""

REPOS_CONTENT = """[repo.stable]
url = "https://packages.skyos.dev/stable/"
enabled = true

[repo.testing]
url = "https://packages.skyos.dev/testing/"
enabled = false
"""

INIT_TOML_CONTENT = """hostname = "skyos"

[[service]]
name = "vahid"
exec = "/bin/vahid"
respawn = true

[[service]]
name = "login-manager"
exec = "/bin/login-manager"
respawn = true

[[service]]
name = "svc"
exec = "/bin/svc"
respawn = true

[[service]]
name = "getty"
exec = "/bin/login"
respawn = true
"""

if __name__ == '__main__':
    root = sys.argv[1] if len(sys.argv) > 1 else '.'
    output = os.path.join(root, 'initrd.tar')
    build_initrd(root, output)
