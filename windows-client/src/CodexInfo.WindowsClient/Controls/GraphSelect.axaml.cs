// Copyright (C) 2026 salty919
// SPDX-License-Identifier: GPL-3.0-only

using System.Collections;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.Primitives;
using Avalonia.Controls.Templates;
using Avalonia.Data;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.VisualTree;

namespace CodexInfo.WindowsClient.Controls;

public partial class GraphSelect : UserControl
{
    public static readonly StyledProperty<string> FieldLabelProperty =
        AvaloniaProperty.Register<GraphSelect, string>(nameof(FieldLabel), "");
    public static readonly StyledProperty<bool> HasFieldLabelProperty =
        AvaloniaProperty.Register<GraphSelect, bool>(nameof(HasFieldLabel));
    public static readonly StyledProperty<Thickness> ValueMarginProperty =
        AvaloniaProperty.Register<GraphSelect, Thickness>(nameof(ValueMargin));
    public static readonly StyledProperty<string> ValueTextProperty =
        AvaloniaProperty.Register<GraphSelect, string>(nameof(ValueText), "");
    public static readonly StyledProperty<IEnumerable?> ItemsSourceProperty =
        AvaloniaProperty.Register<GraphSelect, IEnumerable?>(nameof(ItemsSource));
    public static readonly StyledProperty<object?> SelectedItemProperty =
        AvaloniaProperty.Register<GraphSelect, object?>(nameof(SelectedItem), defaultBindingMode: BindingMode.TwoWay);
    public static readonly StyledProperty<IDataTemplate?> ItemTemplateProperty =
        AvaloniaProperty.Register<GraphSelect, IDataTemplate?>(nameof(ItemTemplate));
    public static readonly StyledProperty<string> MenuAutomationIdProperty =
        AvaloniaProperty.Register<GraphSelect, string>(nameof(MenuAutomationId), "");
    public static readonly StyledProperty<double> PopupWidthProperty =
        AvaloniaProperty.Register<GraphSelect, double>(nameof(PopupWidth), 560);
    public static readonly StyledProperty<int> MaxVisibleItemsProperty =
        AvaloniaProperty.Register<GraphSelect, int>(nameof(MaxVisibleItems), 4);
    public static readonly StyledProperty<bool> LimitPopupToFieldProperty =
        AvaloniaProperty.Register<GraphSelect, bool>(nameof(LimitPopupToField));
    public static readonly StyledProperty<bool> PopupOnLeftProperty =
        AvaloniaProperty.Register<GraphSelect, bool>(nameof(PopupOnLeft));

    public string FieldLabel { get => GetValue(FieldLabelProperty); set => SetValue(FieldLabelProperty, value); }
    public bool HasFieldLabel { get => GetValue(HasFieldLabelProperty); set => SetValue(HasFieldLabelProperty, value); }
    public Thickness ValueMargin { get => GetValue(ValueMarginProperty); set => SetValue(ValueMarginProperty, value); }
    public string ValueText { get => GetValue(ValueTextProperty); set => SetValue(ValueTextProperty, value); }
    public IEnumerable? ItemsSource { get => GetValue(ItemsSourceProperty); set => SetValue(ItemsSourceProperty, value); }
    public object? SelectedItem { get => GetValue(SelectedItemProperty); set => SetValue(SelectedItemProperty, value); }
    public IDataTemplate? ItemTemplate { get => GetValue(ItemTemplateProperty); set => SetValue(ItemTemplateProperty, value); }
    public string MenuAutomationId { get => GetValue(MenuAutomationIdProperty); set => SetValue(MenuAutomationIdProperty, value); }
    public double PopupWidth { get => GetValue(PopupWidthProperty); set => SetValue(PopupWidthProperty, value); }
    public int MaxVisibleItems { get => GetValue(MaxVisibleItemsProperty); set => SetValue(MaxVisibleItemsProperty, value); }
    public bool LimitPopupToField { get => GetValue(LimitPopupToFieldProperty); set => SetValue(LimitPopupToFieldProperty, value); }
    public bool PopupOnLeft { get => GetValue(PopupOnLeftProperty); set => SetValue(PopupOnLeftProperty, value); }

    public bool IsOpen => MenuPopup.IsOpen;

    public GraphSelect()
    {
        InitializeComponent();
        MenuPopup.PlacementTarget = Field;
    }

    private void OnFieldPressed(object? sender, PointerPressedEventArgs e)
    {
        if (!IsEnabled || ItemsSource is null)
        {
            return;
        }

        if (MenuPopup.IsOpen)
        {
            Close();
        }
        else
        {
            var count = ItemsSource.Cast<object>().Count();
            if (count == 0)
            {
                return;
            }

            PopupSurface.Width = LimitPopupToField ? Math.Min(PopupWidth, Bounds.Width) : PopupWidth;
            MenuList.Height = Math.Min(count, MaxVisibleItems) * 32;
            MenuPopup.Placement = PopupOnLeft
                ? PlacementMode.LeftEdgeAlignedTop
                : PlacementMode.BottomEdgeAlignedLeft;
            MenuPopup.IsOpen = true;
            Chevron.Text = "⌃";
            Field.Background = Avalonia.Media.Brush.Parse("#244D74");
            Field.BorderBrush = Avalonia.Media.Brush.Parse("#56B2F5");
        }
        e.Handled = true;
    }

    private void OnSelectionChanged(object? sender, SelectionChangedEventArgs e)
    {
        if (MenuPopup.IsOpen && e.AddedItems.Count > 0)
        {
            Close();
        }
    }

    private void OnMenuPointerReleased(object? sender, PointerReleasedEventArgs e)
    {
        for (var current = e.Source as Visual; current is not null; current = current.GetVisualParent())
        {
            if (current is ListBoxItem item)
            {
                SelectedItem = item.DataContext;
                Close();
                break;
            }
            if (ReferenceEquals(current, MenuList))
            {
                break;
            }
        }
    }

    public void Close()
    {
        MenuPopup.IsOpen = false;
        Chevron.Text = "⌄";
        Field.Background = Avalonia.Media.Brush.Parse("#111B2C");
        Field.BorderBrush = Avalonia.Media.Brush.Parse("#405779");
    }
}
