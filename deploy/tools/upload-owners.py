#!/usr/bin/env python3
"""Tag uploads made before chadd recorded owners (D69), from chadd's own log.

    journalctl -u chadd -o cat | python3 upload-owners.py /var/lib/chadd/uploads

chadd logs `upload account=<a> id=<id> ...` for every upload; this sets the
`user.chad.owner` xattr on `<dir>/<id>/`, as chadd now does itself, so account
deletion finds them. Uploads already tagged are left alone. Older than the log
goes back: they stay unowned and survive account deletion.
"""

import os
import re
import sys

LINE = re.compile(r"\bupload account=(\S+) id=([0-9a-f]{32})\b")
XATTR = "user.chad.owner"


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    root = sys.argv[1]
    tagged = skipped = missing = 0
    for line in sys.stdin:
        m = LINE.search(line)
        if not m:
            continue
        account, upload = m.groups()
        path = os.path.join(root, upload)
        if not os.path.isdir(path):
            missing += 1
            continue
        try:
            os.getxattr(path, XATTR)
            skipped += 1
            continue
        except OSError:
            pass
        os.setxattr(path, XATTR, account.encode())
        tagged += 1
    print(f"tagged {tagged}, already tagged {skipped}, gone {missing}")


if __name__ == "__main__":
    main()
