#include "keys.h"

#include <QtCore/QCoreApplication>
#include <QtCore/QMetaObject>
#include <QtGui/QGuiApplication>
#include <QtGui/QKeyEvent>
#include <QtGui/QWindow>
#include <QtQuick/QQuickItem>
#include <QtQuick/QQuickWindow>

#include <cstdio>

namespace halo {
namespace {
// The combo box whose list is open and has the focus, if that is where the focus is. Its popup is
// a Menu that takes the focus but knows nothing about the combo's items, so the keys that drive
// the list (Up, Down, Space, Return, Escape) are given to the combo box itself, which moves the
// highlight, accepts it or closes the list, as a keyboard does with the Basic style.
QQuickItem *comboOf(QQuickItem *focus)
{
    for (QObject *o = focus; o; o = o->parent()) {
        if (o->inherits("QQuickPopup")) {
            auto *owner = o->property("parent").value<QQuickItem *>();
            return owner && owner->inherits("QQuickComboBox") ? owner : nullptr;
        }
    }
    return nullptr;
}

bool inPopup(QQuickItem *focus)
{
    for (QQuickItem *i = focus; i; i = i->parentItem()) {
        if (i->inherits("QQuickPopupItem")) {
            return true;
        }
    }
    return false;
}
}

void sendKey(std::int32_t key, std::int32_t modifiers)
{
    QCoreApplication *app = QCoreApplication::instance();
    if (!app) {
        return;
    }
    // Queued, so the focus check and the delivery happen on the GUI thread.
    QMetaObject::invokeMethod(
        app,
        [key, modifiers] {
            QWindow *window = QGuiApplication::focusWindow();
            if (!window) {
                return;
            }
            // Up/Down move the focus (Backtab/Tab) unless a popup has it: arrows would stop at
            // Kirigami's ScrollView, which scrolls with them and never lets them reach the page.
            // Inside a popup they stay arrows: a combo's list walks with them and Halo's color
            // picker turns them into Tab itself.
            auto *quick = qobject_cast<QQuickWindow *>(window);
            QQuickItem *focus = quick ? quick->activeFocusItem() : nullptr;
            int sent = key;
            if ((key == Qt::Key_Up || key == Qt::Key_Down) && !inPopup(focus)) {
                sent = key == Qt::Key_Up ? Qt::Key_Backtab : Qt::Key_Tab;
            }
            QObject *target = window;
            const bool drivesList = sent == Qt::Key_Up || sent == Qt::Key_Down || sent == Qt::Key_Space
                || sent == Qt::Key_Return || sent == Qt::Key_Enter || sent == Qt::Key_Escape;
            if (QQuickItem *combo = drivesList ? comboOf(focus) : nullptr) {
                target = combo;
            }
            const auto mods = static_cast<Qt::KeyboardModifiers>(modifiers);
            QKeyEvent press(QEvent::KeyPress, sent, mods);
            QCoreApplication::sendEvent(target, &press);
            QKeyEvent release(QEvent::KeyRelease, sent, mods);
            QCoreApplication::sendEvent(target, &release);
        },
        Qt::QueuedConnection);
}
}

namespace halo {
void debugStep(rust::Str step)
{
    QCoreApplication *app = QCoreApplication::instance();
    if (!app) {
        return;
    }
    const QString text = QString::fromUtf8(step.data(), static_cast<int>(step.size()));
    QMetaObject::invokeMethod(
        app,
        [text] {
            auto *window = qobject_cast<QQuickWindow *>(QGuiApplication::focusWindow());
            if (text == QLatin1String("quit")) {
                QCoreApplication::quit();
            } else if (text == QLatin1String("focus")) {
                QQuickItem *item = window ? window->activeFocusItem() : nullptr;
                std::fprintf(stderr, "halo-gui: focus %s \"%s\"\n",
                             item ? item->metaObject()->className() : "(none)",
                             item ? qPrintable(item->property("text").toString()) : "");
            } else if (text.startsWith(QLatin1String("shot:")) && window) {
                window->grabWindow().save(text.mid(5));
            }
        },
        Qt::QueuedConnection);
}
}
