.PHONY: tags tests

tags:
	ctags -R --exclude=.mypy_cache/* --exclude=*/.mypy_cache/* --exclude=*/*.json \
		--exclude=target/* .

