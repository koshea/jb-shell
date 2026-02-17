from fabric import Application
from fabric.utils import get_relative_path
from widgets.bar import create_bars


def main():
    bars = create_bars()
    app = Application("jb-shell", *bars)
    app.set_stylesheet_from_file(get_relative_path("style.css"))
    app.run()


if __name__ == "__main__":
    main()
